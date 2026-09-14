//! Bridge to the upstream (`@earendil-works/pi`) agent configuration.
//!
//! The TypeScript `pi` stores its state under `~/.pi/agent` (overridable with
//! `PI_CODING_AGENT_DIR`). This module reads that directory so the Rust CLI can
//! reuse the same model selection, credentials, and custom providers instead of
//! keeping a second, disconnected config:
//!
//! - `settings.json` → `defaultProvider` / `defaultModel` / `defaultThinkingLevel`
//! - `auth.json`     → per-provider `{"type":"api_key","key":"..."}`
//! - `models.json`   → custom providers (`baseUrl`, `api`, `apiKey`, `models`)
//!
//! Everything is best-effort: a missing or malformed file is ignored, so the
//! CLI keeps starting when no upstream config is present.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Env var the upstream CLI uses to relocate its agent directory.
const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";

/// Subset of `settings.json` we care about.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub default_thinking_level: Option<String>,
}

/// One entry of `auth.json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthEntry {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub key: Option<String>,
}

/// A model entry declared by a custom provider in `models.json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomModel {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u32>,
    pub max_tokens: Option<u32>,
}

/// A provider declared in `models.json`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomProvider {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api: Option<String>,
    pub api_key: Option<String>,
    pub models: Vec<CustomModel>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ModelsFile {
    providers: HashMap<String, CustomProvider>,
}

/// A model entry from the upstream catalog (`models-store.json` or the
/// bundled `pi-ai` provider data), used for context window / limits / routing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    pub name: Option<String>,
    pub api: Option<String>,
    pub base_url: Option<String>,
    pub context_window: Option<u32>,
    pub max_tokens: Option<u32>,
    pub reasoning: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct StoreProvider {
    models: Vec<CatalogModel>,
}

/// Loaded view of the upstream agent directory.
#[derive(Debug, Clone, Default)]
pub struct AgentHome {
    pub dir: PathBuf,
    pub settings: Settings,
    pub auth: HashMap<String, AuthEntry>,
    pub providers: HashMap<String, CustomProvider>,
    /// provider → model id → catalog entry (from `models-store.json` and the
    /// bundled `pi-ai` provider data).
    pub catalog: HashMap<String, HashMap<String, CatalogModel>>,
}

impl AgentHome {
    /// Locate and load `~/.pi/agent` (or `$PI_CODING_AGENT_DIR`). Returns
    /// `None` when the directory does not exist.
    pub fn load() -> Option<Self> {
        let dir = agent_dir()?;
        if !dir.is_dir() {
            return None;
        }
        let home = Self {
            settings: read_json(&dir.join("settings.json")).unwrap_or_default(),
            auth: read_json(&dir.join("auth.json")).unwrap_or_default(),
            providers: read_json::<ModelsFile>(&dir.join("models.json"))
                .map(|m| m.providers)
                .unwrap_or_default(),
            catalog: load_catalog(&dir),
            dir,
        };
        tracing::debug!(
            dir = %home.dir.display(),
            provider = ?home.settings.default_provider,
            model = ?home.settings.default_model,
            "loaded upstream pi agent config"
        );
        Some(home)
    }

    /// API key for `provider`: `models.json` `apiKey` first, then `auth.json`.
    pub fn api_key(&self, provider: &str) -> Option<String> {
        self.providers
            .get(provider)
            .and_then(|p| p.api_key.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.auth.get(provider).and_then(|entry| {
                    // OAuth entries can be present; only `api_key` (or untyped)
                    // entries hold a usable bearer token.
                    let usable = entry.kind.as_deref().map_or(true, |k| k == "api_key");
                    if usable {
                        entry.key.clone()
                    } else {
                        None
                    }
                })
            })
            .filter(|s| !s.is_empty())
    }

    pub fn custom_provider(&self, name: &str) -> Option<&CustomProvider> {
        self.providers.get(name)
    }

    /// Catalog entry (context window, limits, base URL) for `provider`/`id`.
    pub fn catalog_model(&self, provider: &str, id: &str) -> Option<&CatalogModel> {
        self.catalog.get(provider).and_then(|models| models.get(id))
    }

    /// Find the custom provider (and model) that declares `model_id`.
    pub fn find_custom_model(
        &self,
        model_id: &str,
    ) -> Option<(&str, &CustomProvider, &CustomModel)> {
        for (name, provider) in &self.providers {
            if let Some(model) = provider.models.iter().find(|m| m.id == model_id) {
                return Some((name.as_str(), provider, model));
            }
        }
        None
    }
}

/// Resolve the agent directory: `$PI_CODING_AGENT_DIR` (with `~` expansion)
/// or `~/.pi/agent`.
pub fn agent_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_AGENT_DIR) {
        let expanded = expand_tilde(&dir.to_string_lossy());
        if !expanded.as_os_str().is_empty() {
            return Some(expanded);
        }
    }
    dirs::home_dir().map(|home| home.join(".pi").join("agent"))
}

fn expand_tilde(s: &str) -> PathBuf {
    for prefix in ["~/", "~\\"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest);
            }
        }
    }
    PathBuf::from(s)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Build `provider → id → model` from the cached `models-store.json` and the
/// bundled `pi-ai` provider data (which wins, being the source of truth).
fn load_catalog(agent_dir: &Path) -> HashMap<String, HashMap<String, CatalogModel>> {
    let mut catalog: HashMap<String, HashMap<String, CatalogModel>> = HashMap::new();

    if let Some(store) =
        read_json::<HashMap<String, StoreProvider>>(&agent_dir.join("models-store.json"))
    {
        for (provider, entry) in store {
            let models = catalog.entry(provider).or_default();
            for model in entry.models {
                if !model.id.is_empty() {
                    models.insert(model.id.clone(), model);
                }
            }
        }
    }

    if let Some(data_dir) = find_pi_ai_data_dir() {
        if let Ok(dir) = std::fs::read_dir(&data_dir) {
            for entry in dir.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let Some(provider) = path.file_stem().map(|s| s.to_string_lossy().to_string())
                else {
                    continue;
                };
                // `{ api: { model_id: {...} } }`
                let Some(apis) = read_json::<HashMap<String, HashMap<String, CatalogModel>>>(&path)
                else {
                    continue;
                };
                let models = catalog.entry(provider).or_default();
                for group in apis.into_values() {
                    for (id, model) in group {
                        models.insert(id, model);
                    }
                }
            }
        }
        tracing::debug!(dir = %data_dir.display(), "loaded pi-ai model catalog");
    }

    catalog
}

/// Locate the bundled `pi-ai` provider data by walking `PATH` (the `pi`
/// launcher lives next to `node_modules/@earendil-works/pi-coding-agent`).
fn find_pi_ai_data_dir() -> Option<PathBuf> {
    const REL: &str =
        "node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai/dist/providers/data";
    const REL2: &str = "node_modules/@earendil-works/pi-ai/dist/providers/data";

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for rel in [REL, REL2] {
                let candidate = dir.join(rel);
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let candidate = PathBuf::from(local)
            .join("pi-node")
            .join("current")
            .join(REL);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_settings() {
        let s: Settings = serde_json::from_str(
            r#"{"defaultProvider":"deepseek","defaultModel":"deepseek-v4-flash","defaultThinkingLevel":"high"}"#,
        )
        .unwrap();
        assert_eq!(s.default_provider.as_deref(), Some("deepseek"));
        assert_eq!(s.default_model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(s.default_thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn parses_auth_and_models() {
        let auth: HashMap<String, AuthEntry> =
            serde_json::from_str(r#"{"deepseek":{"type":"api_key","key":"sk-1"}}"#).unwrap();
        assert_eq!(auth["deepseek"].key.as_deref(), Some("sk-1"));

        let models: ModelsFile = serde_json::from_str(
            r#"{"providers":{"empero":{"baseUrl":"https://free.example/v1","api":"openai-completions","apiKey":"free","models":[{"id":"glm-5.3-flash","contextWindow":131072,"maxTokens":16384}]}}}"#,
        )
        .unwrap();
        let p = &models.providers["empero"];
        assert_eq!(p.base_url.as_deref(), Some("https://free.example/v1"));
        assert_eq!(p.models[0].id, "glm-5.3-flash");
        assert_eq!(p.models[0].context_window, Some(131072));
    }

    #[test]
    fn api_key_prefers_models_json() {
        let mut home = AgentHome::default();
        home.auth.insert(
            "deepseek".into(),
            AuthEntry {
                kind: Some("api_key".into()),
                key: Some("from-auth".into()),
            },
        );
        assert_eq!(home.api_key("deepseek").as_deref(), Some("from-auth"));

        home.providers.insert(
            "deepseek".into(),
            CustomProvider {
                api_key: Some("from-models".into()),
                ..Default::default()
            },
        );
        assert_eq!(home.api_key("deepseek").as_deref(), Some("from-models"));
    }

    #[test]
    fn finds_custom_model() {
        let mut home = AgentHome::default();
        home.providers.insert(
            "empero".into(),
            CustomProvider {
                models: vec![CustomModel {
                    id: "glm-5.3-flash".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        let (provider, _, model) = home.find_custom_model("glm-5.3-flash").unwrap();
        assert_eq!(provider, "empero");
        assert_eq!(model.id, "glm-5.3-flash");
        assert!(home.find_custom_model("nope").is_none());
    }
}
