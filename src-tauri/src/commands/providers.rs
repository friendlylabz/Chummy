//! Model connections: presets, add/edit/remove, model lists and testing.

use chummy_llm::{Client, Preset, ProbeReport, ProviderConfig, ProviderKind, presets, probe};
use serde::Serialize;
use tauri::{AppHandle, State, WebviewWindow};

use super::{CmdResult, err};
use crate::commands::local::start_engine_for;
use crate::dialogs;
use crate::state::AppState;

#[tauri::command]
pub fn list_presets() -> Vec<Preset> {
    presets()
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        "model".into()
    } else {
        s.chars().take(40).collect()
    }
}

fn short_random() -> String {
    chummy_store::random_id(3)
}

/// Checks a provider before saving.
fn validate(cfg: &ProviderConfig) -> Result<(), String> {
    if cfg.name.trim().is_empty() {
        return Err("Give this connection a name.".into());
    }
    if cfg.model.trim().is_empty() {
        return Err("Choose a model.".into());
    }
    if cfg.kind != ProviderKind::Local {
        if let Some(why) = chummy_llm::endpoint_problem(&cfg.base_url) {
            return Err(why);
        }
    } else if cfg.local.is_none() {
        return Err("Local models are added by downloading them.".into());
    }
    if let Some(t) = cfg.temperature
        && !(0.0..=2.0).contains(&t)
    {
        return Err("Temperature must be between 0 and 2.".into());
    }
    if !(chummy_llm::MIN_ANSWER_TOKENS..=chummy_llm::MAX_ANSWER_TOKENS).contains(&cfg.max_tokens) {
        return Err("The longest answer must be 16–32,768 tokens.".into());
    }
    if let Some(p) = cfg.generation.problems().into_iter().next() {
        return Err(p);
    }
    if cfg.daily_tokens.is_some_and(|n| n < 1000) {
        return Err("The daily token limit must be at least 1,000 (or turned off).".into());
    }
    let min = chummy_llm::MIN_CONTEXT;
    if cfg.context_tokens.is_some_and(|n| n < min) {
        return Err(format!("The context length must be at least {min} tokens."));
    }
    if cfg.compact_at.is_some_and(|n| n < min) {
        return Err(format!(
            "The compaction point must be at least {min} tokens."
        ));
    }
    Ok(())
}

/// Keeps the user's context settings within what the server reports.
fn cap_context(cfg: &mut ProviderConfig) {
    if let (Some(mine), Some(max)) = (cfg.context_tokens, cfg.context_max) {
        cfg.context_tokens = Some(mine.min(max));
    }
    let window = cfg.context_window();
    if let Some(at) = cfg.compact_at {
        cfg.compact_at = Some(at.min(window));
    }
}

/// Asks the server for the model's context length, giving up after a few
/// seconds (saving shouldn't hang on an unreachable server).
async fn detect_context(cfg: &ProviderConfig, key: Option<String>) -> Option<u32> {
    let client = Client::new(cfg, key).ok()?;
    tokio::time::timeout(std::time::Duration::from_secs(6), client.context_length())
        .await
        .ok()
        .flatten()
}

/// Adds or updates a provider. `api_key`: `None` keeps the stored key,
/// `Some("")` removes it, anything else replaces it. Returns the provider id.
/// A plain `http://` address off the local network is saved only after the
/// user confirms (once per address).
#[tauri::command]
pub async fn save_provider(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    mut config: ProviderConfig,
    api_key: Option<String>,
) -> CmdResult<String> {
    config.name = config.name.trim().to_string();
    config.model = config.model.trim().to_string();
    config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
    validate(&config)?;

    let existing = state.settings();
    let address_changed = existing
        .provider(&config.id)
        .is_none_or(|old| old.base_url != config.base_url);
    if config.kind != ProviderKind::Local
        && address_changed
        && chummy_llm::unencrypted_remote(&config.base_url)
    {
        let msg = format!(
            "{}\n\nThis address uses plain http:// and isn't on this computer or your local network. \
             Your API key and every conversation with this model would travel unencrypted, unless \
             something else protects them, such as a VPN.\n\nUse https:// if the server supports it.",
            config.base_url
        );
        if !dialogs::confirm(
            &app,
            window.label(),
            "Connect without encryption?",
            &msg,
            "Connect anyway",
            true,
        )
        .await
        {
            return Err("Not saved: the address uses unencrypted http://.".into());
        }
        chummy_log::system::log(
            chummy_log::system::Level::Warn,
            "security",
            format!(
                "Model connection {} uses unencrypted http:// off the local network",
                config.name
            ),
            &[("address", &config.base_url)],
        );
    }
    if config.kind != ProviderKind::Local
        && !saved_key_may_go(&app, window.label(), &state, &config, &api_key).await
    {
        return Err(format!("Not saved. {KEY_REFUSED}"));
    }
    let existing = state.settings();
    // Local models are made only by downloading one (commands::local), which
    // decides their folder; a window can't make one up or point an
    // existing one elsewhere.
    if config.kind == ProviderKind::Local
        && existing
            .provider(&config.id)
            .is_none_or(|old| old.kind != ProviderKind::Local)
    {
        return Err("Local models are added by downloading them.".into());
    }
    if config.kind != ProviderKind::Local {
        config.local = None;
    }
    if config.id.is_empty() {
        config.id = format!("{}-{}", slug(&config.name), short_random());
    } else if existing.provider(&config.id).is_none() {
        return Err("That connection no longer exists.".into());
    } else if let Some(old) = existing.provider(&config.id)
        && old.kind == ProviderKind::Local
    {
        // The download determines these; the UI can't change them.
        config.local = old.local.clone();
        config.kind = ProviderKind::Local;
    }

    // The context length the server reports: asked again whenever the
    // address or model changes (a different model has a different one).
    if config.kind != ProviderKind::Local {
        let changed = existing
            .provider(&config.id)
            .is_none_or(|old| old.base_url != config.base_url || old.model != config.model);
        if changed || config.context_max.is_none() {
            let key = key_for(&state, &config, api_key.clone());
            match detect_context(&config, key).await {
                Some(n) => config.context_max = Some(n),
                None if changed => config.context_max = None,
                None => {}
            }
        }
    }
    cap_context(&mut config);

    if let Some(key) = api_key {
        let key = key.trim();
        if key.is_empty() {
            state.secrets.delete(&config.secret_key()).map_err(err)?;
        } else {
            state.secrets.set(&config.secret_key(), key).map_err(err)?;
        }
    }

    let id = config.id.clone();
    state.update(&app, move |s| {
        match s.providers.iter_mut().find(|p| p.id == config.id) {
            Some(p) => *p = config,
            None => s.providers.push(config),
        }
        if s.default_provider
            .as_ref()
            .is_none_or(|d| s.provider(d).is_none())
        {
            s.default_provider = Some(id.clone());
        }
        id
    })
}

#[tauri::command]
pub fn delete_provider(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if let Some(p) = state.settings().provider(&id) {
        let _ = state.secrets.delete(&p.secret_key());
    }
    state.update(&app, |s| {
        s.providers.retain(|p| p.id != id);
        if s.default_provider.as_deref() == Some(id.as_str()) {
            s.default_provider = s.providers.first().map(|p| p.id.clone());
        }
    })
}

#[tauri::command]
pub fn set_default_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<()> {
    if state.settings().provider(&id).is_none() {
        return Err("That connection no longer exists.".into());
    }
    state.update(&app, |s| s.default_provider = Some(id))
}

/// The key to use for a draft: the one typed in, else the stored one.
/// scheme://host:port of an address, to tell whether it's the same server.
fn origin(url: &str) -> Option<String> {
    let u = tauri::Url::parse(url).ok()?;
    Some(format!(
        "{}://{}:{}",
        u.scheme(),
        u.host_str()?.to_ascii_lowercase(),
        u.port_or_known_default()?
    ))
}

/// Before a connection's saved API key goes to a different server than the
/// one it was saved for, asks natively. True: go ahead (same server, no
/// saved key, a key was typed, or the user agreed).
async fn saved_key_may_go(
    app: &AppHandle,
    window: &str,
    state: &AppState,
    cfg: &ProviderConfig,
    typed: &Option<String>,
) -> bool {
    if typed.as_deref().is_some_and(|k| !k.trim().is_empty()) || cfg.id.is_empty() {
        return true;
    }
    let s = state.settings();
    let Some(saved) = s.provider(&cfg.id) else {
        return true;
    };
    if saved.kind == ProviderKind::Local
        || origin(&saved.base_url) == origin(&cfg.base_url)
        || !state.has_secret(&saved.secret_key())
    {
        return true;
    }
    let host = |u: &str| {
        tauri::Url::parse(u)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| u.to_string())
    };
    // Asked once per connection and server until Chummy restarts, so
    // listing models, testing and saving don't each ask again.
    static ALLOWED: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    let pair = (cfg.id.clone(), origin(&cfg.base_url).unwrap_or_default());
    if ALLOWED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&pair)
    {
        return true;
    }
    let (old, new) = (host(&saved.base_url), host(&cfg.base_url));
    let msg = format!(
        "{}\n\nThe API key saved for “{}” was given for {old}. This address is a different server: {new}.\n\n\
         Anyone running that server would get your key. Only send it if you changed the address yourself and \
         trust the new server; otherwise type the key for the new server instead.",
        cfg.base_url, saved.name
    );
    let ok = crate::dialogs::confirm(
        app,
        window,
        &format!("Send your saved API key to {new}?"),
        &msg,
        "Send it",
        true,
    )
    .await;
    if ok {
        ALLOWED.lock().unwrap_or_else(|e| e.into_inner()).push(pair);
        chummy_log::system::log(
            chummy_log::system::Level::Warn,
            "security",
            format!("Saved API key of {} allowed for a new address", saved.name),
            &[("address", &cfg.base_url)],
        );
    }
    ok
}

const KEY_REFUSED: &str = "The saved API key wasn't sent to the new address. Type the key for that server, or change the address back.";

fn key_for(state: &AppState, cfg: &ProviderConfig, typed: Option<String>) -> Option<String> {
    typed.filter(|k| !k.trim().is_empty()).or_else(|| {
        (!cfg.id.is_empty())
            .then(|| state.secrets.get(&cfg.secret_key()).ok().flatten())
            .flatten()
    })
}

/// Lists models offered by a (possibly unsaved) connection.
#[tauri::command]
pub async fn list_models(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    config: ProviderConfig,
    api_key: Option<String>,
) -> CmdResult<Vec<String>> {
    let mut cfg = config;
    if cfg.model.trim().is_empty() {
        cfg.model = "-".into(); // listing doesn't need a model
    }
    if cfg.kind != ProviderKind::Local
        && let Some(why) = chummy_llm::endpoint_problem(&cfg.base_url)
    {
        return Err(why);
    }
    if cfg.kind != ProviderKind::Local
        && !saved_key_may_go(&app, window.label(), &state, &cfg, &api_key).await
    {
        return Err(KEY_REFUSED.into());
    }
    let key = key_for(&state, &cfg, api_key);
    let client = Client::new(&cfg, key).map_err(err)?;
    client.list_models().await.map_err(err)
}

#[derive(Serialize)]
pub struct TestOutcome {
    /// The local engine's runtime isn't installed yet.
    pub needs_runtime: bool,
    pub report: Option<ProbeReport>,
}

/// Runs the connection probe: reach the server, get a reply, call a tool.
#[tauri::command]
pub async fn test_provider(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    config: ProviderConfig,
    api_key: Option<String>,
) -> CmdResult<TestOutcome> {
    let (url, model) = (config.base_url.clone(), config.model.clone());
    let (cfg, key) = if config.kind == ProviderKind::Local {
        let saved = state
            .settings()
            .provider(&config.id)
            .cloned()
            .ok_or("Save the local model first.")?;
        if chummy_local::runtime::status(&state.runtime).is_none() {
            return Ok(TestOutcome {
                needs_runtime: true,
                report: None,
            });
        }
        let endpoint = start_engine_for(&app, &state, &saved).await?;
        let mut cfg = saved;
        cfg.base_url = endpoint.base_url;
        (cfg, Some(endpoint.token))
    } else {
        if let Some(why) = chummy_llm::endpoint_problem(&config.base_url) {
            return Err(why);
        }
        if !saved_key_may_go(&app, window.label(), &state, &config, &api_key).await {
            return Err(KEY_REFUSED.into());
        }
        let key = key_for(&state, &config, api_key);
        (config, key)
    };
    let client = Client::new(&cfg, key).map_err(err)?;
    let report = probe(&client).await;
    // Remember a reported context length for the saved connection.
    if let Some(n) = report.context {
        let id = cfg.id.clone();
        let saved = state.settings().provider(&id).is_some_and(|p| {
            p.context_max != Some(n)
                && p.model == model
                && (p.kind == ProviderKind::Local || p.base_url == url)
        });
        if saved {
            let _ = state.update(&app, move |s| {
                if let Some(p) = s.providers.iter_mut().find(|p| p.id == id) {
                    p.context_max = Some(n);
                    cap_context(p);
                }
            });
        }
    }
    Ok(TestOutcome {
        needs_runtime: false,
        report: Some(report),
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn origins_tell_servers_apart() {
        use super::origin;
        assert_eq!(
            origin("https://api.example.com/v1"),
            origin("https://API.example.com:443/other")
        );
        assert_ne!(
            origin("https://api.example.com/v1"),
            origin("https://evil.example/v1")
        );
        assert_ne!(
            origin("http://10.0.0.2:8000/v1"),
            origin("http://10.0.0.2:8001/v1")
        );
        assert_ne!(origin("http://host/v1"), origin("https://host/v1"));
    }
    use super::*;

    #[test]
    fn slugs_are_safe() {
        assert_eq!(slug("My Local Ollama!"), "my-local-ollama");
        assert_eq!(slug("??"), "model");
    }

    #[test]
    fn validation_catches_common_mistakes() {
        let mut c = ProviderConfig {
            id: String::new(),
            name: "x".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "localhost:8080".into(),
            model: "m".into(),
            local: None,
            temperature: None,
            daily_tokens: Some(chummy_llm::DEFAULT_DAILY_TOKENS),
            max_tokens: chummy_llm::DEFAULT_MAX_TOKENS,
            context_max: None,
            context_tokens: None,
            compact_at: None,
            use_in_app: false,
            generation: Default::default(),
        };
        assert!(validate(&c).is_err());
        c.base_url = "http://localhost:8080/v1".into();
        assert!(validate(&c).is_ok());
        // Plain http off the local network is allowed (saving asks first).
        c.base_url = "http://api.example.com/v1".into();
        assert!(validate(&c).is_ok());
        assert!(chummy_llm::unencrypted_remote(&c.base_url));
        c.base_url = "http://user:key@api.example.com/v1".into();
        assert!(validate(&c).is_err(), "no credentials in addresses");
        c.base_url = "http://localhost:8080/v1".into();
        c.temperature = Some(5.0);
        assert!(validate(&c).is_err());
    }
}
