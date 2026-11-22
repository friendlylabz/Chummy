//! Inspecting Hugging Face repos before downloading them.

use chummy_llm::Precision;
use serde::Serialize;
use serde_json::Value;

use crate::estimate::{Estimates, ModelShape, estimate};
use crate::hardware::Hardware;
use crate::{LocalError, http_client};

pub const HUB: &str = "https://huggingface.co";

/// Files the engine needs, besides the weights.
const SUPPORT_FILES: &[&str] = &[
    "config.json",
    "generation_config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
    "added_tokens.json",
    "tokenizer.model",
    "vocab.json",
    "merges.txt",
    "chat_template.jinja",
    "chat_template.json",
    "model.safetensors.index.json",
];

/// Above this download size the inspection warns that the model is large for
/// the built-in engine.
const LARGE_BYTES: u64 = 30_000_000_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    /// SHA-256 of the content (for files stored with Git LFS / Xet).
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub label: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

/// Everything the "Add local model" screen shows before download.
#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub repo: String,
    pub revision: String,
    pub architecture: Option<String>,
    pub parser: Option<String>,
    pub parser_label: Option<String>,
    pub params: Option<u64>,
    pub context_length: Option<u64>,
    pub files: Vec<RemoteFile>,
    pub download_bytes: u64,
    pub checks: Vec<Check>,
    /// False if any check failed: the model can't be added.
    pub ok: bool,
    pub estimates: Option<Estimates>,
    pub recommended_precision: Precision,
    pub hardware: Hardware,
}

/// Validates a repo id (or a huggingface.co URL) and returns `owner/name`.
/// The result is safe to use in URLs and folder names.
pub fn normalize_repo_id(input: &str) -> Result<String, LocalError> {
    let s = input.trim().trim_end_matches('/');
    let s = s
        .strip_prefix("https://huggingface.co/")
        .or_else(|| s.strip_prefix("http://huggingface.co/"))
        .or_else(|| s.strip_prefix("huggingface.co/"))
        .unwrap_or(s);
    // Drop "/tree/main" and similar URL tails.
    let mut parts = s.split('/');
    let (Some(owner), Some(name)) = (parts.next(), parts.next()) else {
        return Err(LocalError::BadRepoId(input.into()));
    };
    let ok = |p: &str| {
        !p.is_empty()
            && p.len() <= 96
            && !p.starts_with('.')
            && !p.contains("..")
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !ok(owner) || !ok(name) {
        return Err(LocalError::BadRepoId(input.into()));
    }
    Ok(format!("{owner}/{name}"))
}

/// Folder name for a repo inside `models/`.
pub fn dir_name(repo: &str) -> String {
    repo.replace('/', "--")
}

pub struct Hub {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Hub {
    /// The public Hub, or a mirror set with the standard `HF_ENDPOINT`
    /// environment variable.
    pub fn new(token: Option<String>) -> Self {
        let base = std::env::var("HF_ENDPOINT")
            .ok()
            .filter(|s| s.starts_with("https://") || s.starts_with("http://"))
            .unwrap_or_else(|| HUB.to_string());
        Self::with_base(&base, token)
    }

    pub fn with_base(base: &str, token: Option<String>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.filter(|t| !t.trim().is_empty()),
            http: http_client(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn file_url(&self, repo: &str, revision: &str, path: &str) -> String {
        format!("{}/{repo}/resolve/{revision}/{path}", self.base)
    }

    pub(crate) fn request(&self, url: &str) -> reqwest::RequestBuilder {
        let r = self.http.get(url);
        match &self.token {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
    }

    /// GETs a URL. `Ok(None)` on 404.
    async fn get(&self, url: &str) -> Result<Option<String>, LocalError> {
        let resp = self
            .request(url)
            .send()
            .await
            .map_err(|_| LocalError::Network(self.base.clone()))?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => Ok(Some(
                resp.text()
                    .await
                    .map_err(|_| LocalError::Network(self.base.clone()))?,
            )),
            404 => Ok(None),
            401 | 403 if self.token.is_some() => {
                // With a token, 403 usually means the license wasn't accepted.
                if status == 403 {
                    Err(LocalError::NeedsAccess)
                } else {
                    Err(LocalError::BadToken(status))
                }
            }
            401 | 403 => Err(LocalError::NeedsAccess),
            _ => Err(LocalError::Http {
                status,
                url: url.into(),
            }),
        }
    }

    async fn get_json(&self, url: &str) -> Result<Option<Value>, LocalError> {
        match self.get(url).await? {
            Some(t) => serde_json::from_str(&t)
                .map(Some)
                .map_err(|e| LocalError::Check(format!("unexpected reply from Hugging Face: {e}"))),
            None => Ok(None),
        }
    }

    /// Looks a repo up and decides whether the built-in engine can run it.
    pub async fn inspect(&self, input: &str, hw: &Hardware) -> Result<Inspection, LocalError> {
        let repo = normalize_repo_id(input)?;
        let info = self
            .get_json(&format!("{}/api/models/{repo}", self.base))
            .await?
            .ok_or_else(|| LocalError::NotFound(repo.clone()))?;
        let revision = info["sha"].as_str().unwrap_or("main").to_string();

        let tree = self
            .get_json(&format!("{}/api/models/{repo}/tree/{revision}", self.base))
            .await?
            .unwrap_or(Value::Array(vec![]));

        // Gated repos let anyone read metadata but not files.
        let (config, access_error) = match self
            .get_json(&self.file_url(&repo, &revision, "config.json"))
            .await
        {
            Ok(c) => (c, None),
            Err(e @ (LocalError::NeedsAccess | LocalError::BadToken(_))) => {
                (None, Some(e.to_string()))
            }
            Err(e) => return Err(e),
        };

        let mut template = None;
        if access_error.is_none() {
            if let Some(tc) = self
                .get_json(&self.file_url(&repo, &revision, "tokenizer_config.json"))
                .await?
            {
                template = template_from_tokenizer_config(&tc);
            }
            let has_jinja = tree
                .as_array()
                .is_some_and(|a| a.iter().any(|f| f["path"] == "chat_template.jinja"));
            if template.is_none() && has_jinja {
                template = self
                    .get(&self.file_url(&repo, &revision, "chat_template.jinja"))
                    .await?;
            }
        }

        Ok(analyze(
            Facts {
                repo,
                revision,
                info,
                tree,
                config,
                template,
                access_error,
            },
            hw,
        ))
    }
}

/// Picks the chat template out of `tokenizer_config.json`, preferring a
/// dedicated tool-use template when there are several.
fn template_from_tokenizer_config(tc: &Value) -> Option<String> {
    match &tc["chat_template"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(list) => {
            let by_name = |n: &str| {
                list.iter()
                    .find(|t| t["name"] == n)
                    .and_then(|t| t["template"].as_str())
            };
            by_name("tool_use")
                .or_else(|| by_name("default"))
                .or_else(|| list.first().and_then(|t| t["template"].as_str()))
                .map(str::to_string)
        }
        _ => None,
    }
}

/// Recognizes a model's tool-call format from its chat template.
pub fn detect_parser(template: &str) -> Option<(&'static str, &'static str)> {
    if template.contains("<tool_call>") {
        Some(("hermes", "<tool_call> tags (Qwen / Hermes style)"))
    } else if template.contains("[TOOL_CALLS]") {
        Some(("mistral", "[TOOL_CALLS] (Mistral style)"))
    } else if template.contains("<|tool_call|>") {
        Some(("granite", "<|tool_call|> (Granite / Phi style)"))
    } else if template.contains("<|python_tag|>")
        || (template.contains("ipython") && template.contains("parameters"))
    {
        Some(("llama3_json", "JSON calls (Llama 3 style)"))
    } else {
        None
    }
}

/// Raw facts gathered from the Hub.
pub(crate) struct Facts {
    pub repo: String,
    pub revision: String,
    pub info: Value,
    pub tree: Value,
    pub config: Option<Value>,
    pub template: Option<String>,
    pub access_error: Option<String>,
}

fn is_weight_file(path: &str) -> bool {
    path.ends_with(".safetensors") && path.starts_with("model")
}

pub(crate) fn analyze(f: Facts, hw: &Hardware) -> Inspection {
    use CheckStatus::*;
    let mut checks = Vec::new();
    let mut check = |label, status, detail: String| {
        checks.push(Check {
            label,
            status,
            detail,
        })
    };

    // Top-level files only; subfolders hold alternative formats (onnx/, original/, …).
    let all: Vec<RemoteFile> = f
        .tree
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "file")
        .filter_map(|e| {
            let path = e["path"].as_str()?.to_string();
            if path.contains('/') {
                return None;
            }
            let size = e["lfs"]["size"]
                .as_u64()
                .or_else(|| e["size"].as_u64())
                .unwrap_or(0);
            let sha256 = e["lfs"]["oid"].as_str().map(str::to_string);
            Some(RemoteFile { path, size, sha256 })
        })
        .collect();

    let weights: Vec<&RemoteFile> = all.iter().filter(|x| is_weight_file(&x.path)).collect();
    let has_pickle = all
        .iter()
        .any(|x| x.path.ends_with(".bin") || x.path.ends_with(".pt") || x.path.ends_with(".pth"));
    let files: Vec<RemoteFile> = all
        .iter()
        .filter(|x| is_weight_file(&x.path) || SUPPORT_FILES.contains(&x.path.as_str()))
        .cloned()
        .collect();
    let download_bytes: u64 = files.iter().map(|x| x.size).sum();

    // Access.
    let gated = !matches!(f.info["gated"], Value::Bool(false) | Value::Null);
    match &f.access_error {
        Some(e) => check("Access", Fail, e.clone()),
        None if gated => check("Access", Pass, "Gated model; your token has access.".into()),
        None => check("Access", Pass, "Public model.".into()),
    }

    let cfg = f.config.clone().unwrap_or(Value::Null);
    // Some configs nest the language model under text_config.
    let text_cfg = if cfg["text_config"].is_object() {
        &cfg["text_config"]
    } else {
        &cfg
    };
    let arch = cfg["architectures"][0].as_str().map(str::to_string);

    // Text model?
    if f.access_error.is_none() {
        let pipeline = f.info["pipeline_tag"].as_str().unwrap_or_default();
        let causal = arch.as_deref().is_some_and(|a| a.ends_with("ForCausalLM"));
        let vision = cfg.get("vision_config").is_some_and(|v| !v.is_null());
        if causal && !vision {
            check(
                "Text language model",
                Pass,
                format!(
                    "{} ({}).",
                    arch.clone().unwrap_or_default(),
                    if pipeline.is_empty() {
                        "text-generation"
                    } else {
                        pipeline
                    }
                ),
            );
        } else if arch.is_none() {
            check(
                "Text language model",
                Fail,
                "No model architecture in config.json.".into(),
            );
        } else {
            check(
                "Text language model",
                Fail,
                format!(
                    "{} isn't a plain text-generation model{}. The built-in engine only runs *ForCausalLM models.",
                    arch.clone().unwrap_or_default(),
                    if vision {
                        " (it includes a vision encoder)"
                    } else {
                        ""
                    }
                ),
            );
        }

        // Custom code?
        if cfg.get("auto_map").is_some_and(|v| !v.is_null()) {
            check("No custom code", Fail, "This model needs to run code from its repo (trust_remote_code). Chummy won't run downloaded code.".into());
        } else {
            check(
                "No custom code",
                Pass,
                "Uses the standard transformers implementation.".into(),
            );
        }

        // Pre-quantized?
        if let Some(q) = cfg.get("quantization_config").filter(|v| !v.is_null()) {
            let method = q["quant_method"].as_str().unwrap_or("unknown");
            check(
                "Unquantized weights",
                Warn,
                format!("Pre-quantized ({method}); it may need extra packages and might not load."),
            );
        }
    }

    // Weights.
    if !weights.is_empty() {
        check(
            "Safe weight format",
            Pass,
            format!(
                "{} safetensors file{}.",
                weights.len(),
                if weights.len() == 1 { "" } else { "s" }
            ),
        );
    } else if has_pickle {
        check("Safe weight format", Fail, "Only PyTorch pickle (.bin) weights, which can run code when loaded. Chummy only loads safetensors.".into());
    } else {
        check(
            "Safe weight format",
            Fail,
            "No model weights found at the top of the repo.".into(),
        );
    }

    // Template + tools + parser.
    let mut parser = None;
    if f.access_error.is_none() {
        match &f.template {
            None => {
                check(
                    "Chat template",
                    Fail,
                    "No chat template, so it isn't set up as a chat model.".into(),
                );
            }
            Some(t) => {
                check("Chat template", Pass, "Found.".into());
                if t.contains("tools") {
                    check(
                        "Tool calling",
                        Pass,
                        "The chat template accepts tool definitions.".into(),
                    );
                } else {
                    check("Tool calling", Fail, "The chat template doesn't accept tools, so agents couldn't use tools with it.".into());
                }
                match detect_parser(t) {
                    Some((id, label)) => {
                        parser = Some((id, label));
                        check(
                            "Tool-call parser",
                            Pass,
                            format!("Chummy can read its tool calls: {label}."),
                        );
                    }
                    None => check(
                        "Tool-call parser",
                        Fail,
                        "Chummy doesn't recognise this model's tool-call format.".into(),
                    ),
                }
            }
        }
    }

    // Size.
    if download_bytes > LARGE_BYTES {
        check(
            "Size",
            Warn,
            format!(
                "{:.0} GB is large for the built-in engine; consider vLLM or llama.cpp.",
                download_bytes as f64 / 1e9
            ),
        );
    }

    // Shape for estimates.
    let n = |v: &Value| v.as_u64().filter(|&x| x > 0);
    let dtype_bytes = match cfg["torch_dtype"].as_str().or(cfg["dtype"].as_str()) {
        Some("float32") => 4,
        _ => 2,
    };
    let weight_bytes: u64 = weights.iter().map(|w| w.size).sum();
    let params = n(&f.info["safetensors"]["total"]).or(if weight_bytes > 0 {
        Some(weight_bytes / dtype_bytes)
    } else {
        None
    });
    let heads = n(&text_cfg["num_attention_heads"]);
    let hidden = n(&text_cfg["hidden_size"]);
    let shape = params.map(|params| ModelShape {
        params,
        layers: n(&text_cfg["num_hidden_layers"]).unwrap_or(0),
        kv_heads: n(&text_cfg["num_key_value_heads"]).or(heads).unwrap_or(0),
        head_dim: n(&text_cfg["head_dim"])
            .or_else(|| Some(hidden? / heads?))
            .unwrap_or(0),
        max_context: n(&text_cfg["max_position_embeddings"]).unwrap_or(0),
    });
    let estimates = shape.map(|s| estimate(s, hw));
    let recommended_precision = estimates
        .as_ref()
        .and_then(|e| e.fits.first())
        .and_then(|fit| fit.best.as_ref())
        .map(|c| c.precision)
        .unwrap_or_default();

    let ok = !checks.iter().any(|c| c.status == Fail);
    Inspection {
        repo: f.repo,
        revision: f.revision,
        architecture: arch,
        parser: parser.map(|p| p.0.to_string()),
        parser_label: parser.map(|p| p.1.to_string()),
        params,
        context_length: shape.map(|s| s.max_context).filter(|&c| c > 0),
        files,
        download_bytes,
        checks,
        ok,
        estimates,
        recommended_precision,
        hardware: hw.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hw() -> Hardware {
        Hardware {
            os: "linux",
            arch: "x86_64",
            ram_bytes: 32_000_000_000,
            nvidia: vec![],
            apple_silicon: false,
        }
    }

    fn qwen_like() -> Facts {
        Facts {
            repo: "Qwen/Qwen2.5-1.5B-Instruct".into(),
            revision: "abc123".into(),
            info: json!({ "sha": "abc123", "pipeline_tag": "text-generation", "gated": false,
                          "safetensors": { "total": 1_543_714_304u64 } }),
            tree: json!([
                { "type": "file", "path": "config.json", "size": 660 },
                { "type": "file", "path": "tokenizer.json", "size": 7_031_645 },
                { "type": "file", "path": "tokenizer_config.json", "size": 7_305 },
                { "type": "file", "path": "model.safetensors", "size": 3_087_467_144u64,
                  "lfs": { "oid": "deadbeef", "size": 3_087_467_144u64 } },
                { "type": "file", "path": "README.md", "size": 4_000 },
                { "type": "file", "path": "onnx/model.onnx", "size": 1 },
                { "type": "directory", "path": "onnx" }
            ]),
            config: Some(
                json!({ "architectures": ["Qwen2ForCausalLM"], "torch_dtype": "bfloat16",
                                  "num_hidden_layers": 28, "num_attention_heads": 12, "num_key_value_heads": 2,
                                  "hidden_size": 1536, "max_position_embeddings": 32768 }),
            ),
            template: Some("{% if tools %}...<tool_call>...{% endif %}".into()),
            access_error: None,
        }
    }

    #[test]
    fn accepts_a_good_model() {
        let i = analyze(qwen_like(), &hw());
        assert!(i.ok, "{:?}", i.checks);
        assert_eq!(i.parser.as_deref(), Some("hermes"));
        assert_eq!(i.files.len(), 4, "README and onnx/ are skipped");
        assert_eq!(i.download_bytes, 660 + 7_031_645 + 7_305 + 3_087_467_144);
        let s = i.estimates.unwrap().shape;
        assert_eq!((s.layers, s.kv_heads, s.head_dim), (28, 2, 128));
        assert_eq!(
            i.files
                .iter()
                .find(|f| f.path == "model.safetensors")
                .unwrap()
                .sha256
                .as_deref(),
            Some("deadbeef")
        );
    }

    #[test]
    fn rejects_custom_code_pickles_vision_and_toolless_templates() {
        let mut f = qwen_like();
        f.config.as_mut().unwrap()["auto_map"] = json!({ "AutoModelForCausalLM": "x.Y" });
        assert!(!analyze(f, &hw()).ok);

        let mut f = qwen_like();
        f.tree = json!([{ "type": "file", "path": "pytorch_model.bin", "size": 10 }]);
        let i = analyze(f, &hw());
        assert!(
            i.checks
                .iter()
                .any(|c| c.label == "Safe weight format" && c.status == CheckStatus::Fail)
        );

        let mut f = qwen_like();
        f.config = Some(
            json!({ "architectures": ["Qwen2VLForConditionalGeneration"], "vision_config": {} }),
        );
        assert!(!analyze(f, &hw()).ok);

        let mut f = qwen_like();
        f.template = Some("{{ messages }}".into());
        let i = analyze(f, &hw());
        assert!(!i.ok);
        assert!(i.parser.is_none());
    }

    #[test]
    fn gated_without_access_fails_cleanly() {
        let mut f = qwen_like();
        f.config = None;
        f.template = None;
        f.access_error = Some(LocalError::NeedsAccess.to_string());
        let i = analyze(f, &hw());
        assert!(!i.ok);
        assert_eq!(i.checks[0].label, "Access");
    }

    #[test]
    fn repo_ids_are_validated() {
        assert_eq!(
            normalize_repo_id(" Qwen/Qwen2.5-1.5B-Instruct ").unwrap(),
            "Qwen/Qwen2.5-1.5B-Instruct"
        );
        assert_eq!(
            normalize_repo_id("https://huggingface.co/meta-llama/Llama-3.2-3B-Instruct/tree/main")
                .unwrap(),
            "meta-llama/Llama-3.2-3B-Instruct"
        );
        for bad in [
            "",
            "noslash",
            "../etc/passwd",
            "a/..",
            "a/b c",
            ".hidden/x",
            "a/b%2f",
        ] {
            assert!(normalize_repo_id(bad).is_err(), "{bad}");
        }
        assert_eq!(dir_name("a/b"), "a--b");
    }

    #[test]
    fn detects_parsers() {
        assert_eq!(detect_parser("x [TOOL_CALLS] y").unwrap().0, "mistral");
        assert_eq!(detect_parser("<|python_tag|>").unwrap().0, "llama3_json");
        assert_eq!(detect_parser("<|tool_call|>").unwrap().0, "granite");
        assert!(detect_parser("plain").is_none());
    }

    #[test]
    fn picks_tool_use_template() {
        let tc = json!({ "chat_template": [ { "name": "default", "template": "a" }, { "name": "tool_use", "template": "b" } ] });
        assert_eq!(template_from_tokenizer_config(&tc).as_deref(), Some("b"));
    }
}
