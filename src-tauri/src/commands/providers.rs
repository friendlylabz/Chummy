//! Model connections: presets, add/edit/remove, model lists and testing.

use chummy_llm::{Client, Preset, ProbeReport, ProviderConfig, ProviderKind, presets, probe};
use serde::Serialize;
use tauri::{AppHandle, State};

use super::{CmdResult, err};
use crate::commands::local::start_engine_for;
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
    let mut b = [0u8; 3];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
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
    if cfg.daily_tokens.is_some_and(|n| n < 1000) {
        return Err("The daily token limit must be at least 1,000 (or turned off).".into());
    }
    Ok(())
}

/// Adds or updates a provider. `api_key`: `None` keeps the stored key,
/// `Some("")` removes it, anything else replaces it. Returns the provider id.
#[tauri::command]
pub fn save_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    mut config: ProviderConfig,
    api_key: Option<String>,
) -> CmdResult<String> {
    config.name = config.name.trim().to_string();
    config.model = config.model.trim().to_string();
    config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
    validate(&config)?;

    let existing = state.settings();
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
    state: State<'_, AppState>,
    config: ProviderConfig,
    api_key: Option<String>,
) -> CmdResult<TestOutcome> {
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
        let key = key_for(&state, &config, api_key);
        (config, key)
    };
    let client = Client::new(&cfg, key).map_err(err)?;
    Ok(TestOutcome {
        needs_runtime: false,
        report: Some(probe(&client).await),
    })
}

#[cfg(test)]
mod tests {
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
        };
        assert!(validate(&c).is_err());
        c.base_url = "http://localhost:8080/v1".into();
        assert!(validate(&c).is_ok());
        c.base_url = "http://api.example.com/v1".into();
        assert!(validate(&c).is_err(), "no plain http to the internet");
        c.base_url = "http://localhost:8080/v1".into();
        c.temperature = Some(5.0);
        assert!(validate(&c).is_err());
    }
}
