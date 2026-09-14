use std::path::PathBuf;

use pi_ai::{Model, ModelPricing, ThinkingLevel};

use crate::pi_agent_config::AgentHome;

pub const APP_NAME: &str = "pi";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub model: Model,
    pub max_turns: u32,
    pub thinking_level: ThinkingLevel,
    pub config_dir: PathBuf,
    /// API key resolved from the upstream `~/.pi/agent` config, if any. It is
    /// injected into `StreamOptions` so providers prefer it over their env var.
    pub api_key: Option<String>,
    /// `--system-prompt`: replaces the default system prompt.
    pub system_prompt: Option<String>,
    /// `--append-system-prompt` (repeatable).
    pub system_prompt_append: Vec<String>,
    /// `--no-context-files`: skip AGENTS.md / CLAUDE.md discovery.
    pub no_context_files: bool,
    /// Summarize the abandoned branch when `/tree` switches branches.
    /// Used by the `tui` feature.
    #[allow(dead_code)]
    pub summarize_branches: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let config_dir = dirs::config_dir()
            .map(|p| p.join(APP_NAME))
            .unwrap_or_else(|| PathBuf::from(".pi"));
        Self {
            model: default_model_from_env(),
            max_turns: 32,
            thinking_level: ThinkingLevel::Off,
            config_dir,
            api_key: None,
            system_prompt: None,
            system_prompt_append: Vec::new(),
            no_context_files: false,
            summarize_branches: true,
        }
    }
}

/// A resolved model together with the API key to use for it.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub model: Model,
    pub api_key: Option<String>,
}

/// Parse a `thinking_level` string from the file config into a
/// [`ThinkingLevel`]. Unknown values return `None`.
pub fn parse_thinking_level(s: &str) -> Option<ThinkingLevel> {
    match s.to_ascii_lowercase().as_str() {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" | "max" => Some(ThinkingLevel::Xhigh),
        _ => None,
    }
}

/// Map a well-known model alias to a built-in model constructor.
fn alias_model(id: &str) -> Option<Model> {
    match id {
        "claude-sonnet-4-6" | "claude-sonnet" | "sonnet" => {
            Some(Model::anthropic_claude_sonnet_4_6())
        }
        "claude-opus-4-7" | "claude-opus" | "opus" => Some(Model::anthropic_claude_opus_4_7()),
        "gpt-4o" => Some(Model::openai_gpt_4o()),
        "gpt-4o-mini" => Some(Model::openai_gpt_4o_mini()),
        "gemini-2.0-flash" | "gemini" => Some(Model::gemini_2_0_flash()),
        _ => None,
    }
}

/// Built-in `(api, base_url)` for providers the upstream `pi` knows about but
/// this port does not expose as dedicated constructors.
fn builtin_provider(provider: &str) -> Option<(&'static str, &'static str)> {
    let (api, base_url) = match provider {
        "anthropic" => ("anthropic-messages", "https://api.anthropic.com"),
        "openai" => ("openai-completions", "https://api.openai.com/v1"),
        "google" => (
            "google-generative-ai",
            "https://generativelanguage.googleapis.com",
        ),
        "deepseek" => ("openai-completions", "https://api.deepseek.com"),
        "openrouter" => ("openai-completions", "https://openrouter.ai/api/v1"),
        "groq" => ("openai-completions", "https://api.groq.com/openai/v1"),
        "xai" => ("openai-completions", "https://api.x.ai/v1"),
        "mistral" => ("openai-completions", "https://api.mistral.ai/v1"),
        "together" => ("openai-completions", "https://api.together.xyz/v1"),
        "fireworks" => (
            "openai-completions",
            "https://api.fireworks.ai/inference/v1",
        ),
        "cerebras" => ("openai-completions", "https://api.cerebras.ai/v1"),
        "moonshotai" => ("openai-completions", "https://api.moonshot.ai/v1"),
        "moonshotai-cn" => ("openai-completions", "https://api.moonshot.cn/v1"),
        "kimi-coding" => ("anthropic-messages", "https://api.kimi.com/coding"),
        _ => return None,
    };
    Some((api, base_url))
}

/// Resolve the model the CLI should run.
///
/// Precedence:
/// 1. `explicit` (the `-m` flag, `PI_MODEL`, or `config.toml`) that names a
///    built-in alias.
/// 2. A model declared by a custom provider in the upstream `models.json`.
/// 3. The upstream `settings.json` `defaultProvider` / `defaultModel`.
/// 4. The original env-based fallback ([`default_model_from_env`]).
pub fn resolve_model(explicit: Option<&str>, home: Option<&AgentHome>) -> ResolvedModel {
    let explicit = explicit.map(str::trim).filter(|s| !s.is_empty());

    if let Some(id) = explicit {
        if let Some(model) = alias_model(id) {
            let api_key = home.and_then(|h| h.api_key(&model.provider));
            return ResolvedModel { model, api_key };
        }
    }

    if let Some(home) = home {
        if let Some(resolved) = resolve_from_home(explicit, home) {
            return resolved;
        }
    }

    // Preserve the pre-existing behavior for callers without an upstream config.
    let model = match explicit.and_then(alias_model) {
        Some(model) => model,
        None => default_model_from_env(),
    };
    let api_key = home.and_then(|h| h.api_key(&model.provider));
    ResolvedModel { model, api_key }
}

/// Try to build a model from the upstream provider catalog.
fn resolve_from_home(explicit: Option<&str>, home: &AgentHome) -> Option<ResolvedModel> {
    let (provider, model_id) = match explicit {
        Some(id) => {
            if let Some((name, _, _)) = home.find_custom_model(id) {
                (name.to_string(), id.to_string())
            } else if home.settings.default_model.as_deref() == Some(id) {
                (home.settings.default_provider.clone()?, id.to_string())
            } else {
                // Explicit model id, unknown provider → assume it belongs to
                // the configured default provider.
                let provider = home.settings.default_provider.clone()?;
                (provider, id.to_string())
            }
        }
        None => (
            home.settings.default_provider.clone()?,
            home.settings.default_model.clone()?,
        ),
    };

    let custom = home.custom_provider(&provider);
    let catalog = home.catalog_model(&provider, &model_id);
    let builtin = builtin_provider(&provider);
    let api = custom
        .and_then(|p| non_empty(p.api.as_deref()))
        .or_else(|| catalog.and_then(|m| non_empty(m.api.as_deref())))
        .or_else(|| builtin.map(|(api, _)| api.to_string()))?;
    let base_url = custom
        .and_then(|p| non_empty(p.base_url.as_deref()))
        .or_else(|| catalog.and_then(|m| non_empty(m.base_url.as_deref())))
        .or_else(|| builtin.map(|(_, url)| url.to_string()))?;

    let custom_model = custom.and_then(|p| p.models.iter().find(|m| m.id == model_id));
    let context_window = catalog
        .and_then(|m| m.context_window)
        .or_else(|| custom_model.and_then(|m| m.context_window))
        .unwrap_or(200_000);
    let max_tokens = catalog
        .and_then(|m| m.max_tokens)
        .or_else(|| custom_model.and_then(|m| m.max_tokens))
        .unwrap_or(8_192);

    let model = build_model(
        &provider,
        &model_id,
        &api,
        &base_url,
        context_window,
        max_tokens,
    );
    let api_key = home.api_key(&provider);
    Some(ResolvedModel { model, api_key })
}

/// Built-in models the port knows how to run without any upstream config.
pub fn builtin_models() -> Vec<Model> {
    vec![
        Model::anthropic_claude_sonnet_4_6(),
        Model::anthropic_claude_opus_4_7(),
        Model::openai_gpt_4o(),
        Model::openai_gpt_4o_mini(),
        Model::openai_gpt_5(),
        Model::gemini_2_0_flash(),
    ]
}

/// `provider/model` ids available from the built-ins plus the upstream config,
/// filtered by a case-insensitive substring `search` and sorted.
pub fn list_models(search: &str, home: Option<&AgentHome>) -> Vec<String> {
    let mut entries: Vec<String> = builtin_models()
        .into_iter()
        .map(|m| format!("{}/{}", m.provider, m.id))
        .collect();

    if let Some(home) = home {
        if let (Some(provider), Some(model)) = (
            &home.settings.default_provider,
            &home.settings.default_model,
        ) {
            entries.push(format!("{provider}/{model}"));
        }
        for (name, provider) in &home.providers {
            for model in &provider.models {
                entries.push(format!("{name}/{}", model.id));
            }
        }
    }

    let needle = search.trim().to_lowercase();
    let mut seen = std::collections::BTreeSet::new();
    entries.retain(|entry| {
        seen.insert(entry.clone()) && (needle.is_empty() || entry.to_lowercase().contains(&needle))
    });
    entries.sort();
    entries
}

/// Build a model for an explicitly requested provider (`--provider`).
///
/// `api` + base URL come from the upstream `models.json` custom provider or the
/// built-in table; the model id is `--model` if given, else the custom
/// provider's first model, else the upstream default model when it belongs to
/// this provider.
pub fn resolve_provider(
    provider: &str,
    explicit_model: Option<&str>,
    home: Option<&AgentHome>,
) -> Option<ResolvedModel> {
    let custom = home.and_then(|h| h.custom_provider(provider));
    let builtin = builtin_provider(provider);

    let model_id = explicit_model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| custom.and_then(|p| p.models.first().map(|m| m.id.clone())))
        .or_else(|| {
            home.and_then(|h| {
                let settings = &h.settings;
                match (
                    settings.default_provider.as_deref(),
                    settings.default_model.as_deref(),
                ) {
                    (Some(p), Some(m)) if p == provider => Some(m.to_string()),
                    _ => None,
                }
            })
        })?;

    let catalog = home.and_then(|h| h.catalog_model(provider, &model_id));
    let api = custom
        .and_then(|p| non_empty(p.api.as_deref()))
        .or_else(|| catalog.and_then(|m| non_empty(m.api.as_deref())))
        .or_else(|| builtin.map(|(api, _)| api.to_string()))?;
    let base_url = custom
        .and_then(|p| non_empty(p.base_url.as_deref()))
        .or_else(|| catalog.and_then(|m| non_empty(m.base_url.as_deref())))
        .or_else(|| builtin.map(|(_, url)| url.to_string()))?;

    let custom_model = custom.and_then(|p| p.models.iter().find(|m| m.id == model_id));
    let context_window = catalog
        .and_then(|m| m.context_window)
        .or_else(|| custom_model.and_then(|m| m.context_window))
        .unwrap_or(200_000);
    let max_tokens = catalog
        .and_then(|m| m.max_tokens)
        .or_else(|| custom_model.and_then(|m| m.max_tokens))
        .unwrap_or(8_192);

    let model = build_model(
        provider,
        &model_id,
        &api,
        &base_url,
        context_window,
        max_tokens,
    );
    let api_key = home.and_then(|h| h.api_key(provider));
    Some(ResolvedModel { model, api_key })
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn build_model(
    provider: &str,
    id: &str,
    api: &str,
    base_url: &str,
    context_window: u32,
    max_tokens: u32,
) -> Model {
    Model {
        id: id.to_string(),
        name: id.to_string(),
        api: api.to_string(),
        provider: provider.to_string(),
        base_url: base_url.to_string(),
        reasoning: true,
        context_window,
        max_tokens,
        pricing: ModelPricing::default(),
    }
}

/// Default model derived from `PI_MODEL` and the provider API keys present in
/// the environment. Kept for `AppConfig::default` and as the final fallback.
pub fn default_model_from_env() -> Model {
    if let Ok(id) = std::env::var("PI_MODEL") {
        if let Some(model) = alias_model(&id) {
            return model;
        }
    }
    if std::env::var("GOOGLE_API_KEY").is_ok() || std::env::var("GEMINI_API_KEY").is_ok() {
        Model::gemini_2_0_flash()
    } else if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        Model::anthropic_claude_sonnet_4_6()
    } else if std::env::var("OPENAI_API_KEY").is_ok() {
        Model::openai_gpt_4o_mini()
    } else {
        Model::anthropic_claude_sonnet_4_6()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_agent_config::{AuthEntry, CustomProvider, Settings};

    fn home_with_deepseek() -> AgentHome {
        let mut home = AgentHome {
            settings: Settings {
                default_provider: Some("deepseek".into()),
                default_model: Some("deepseek-v4-flash".into()),
                default_thinking_level: Some("high".into()),
            },
            ..Default::default()
        };
        home.auth.insert(
            "deepseek".into(),
            AuthEntry {
                kind: Some("api_key".into()),
                key: Some("sk-deepseek".into()),
            },
        );
        home
    }

    #[test]
    fn resolves_default_from_upstream_settings() {
        let home = home_with_deepseek();
        let resolved = resolve_model(None, Some(&home));
        assert_eq!(resolved.model.provider, "deepseek");
        assert_eq!(resolved.model.id, "deepseek-v4-flash");
        assert_eq!(resolved.model.api, "openai-completions");
        assert_eq!(resolved.model.base_url, "https://api.deepseek.com");
        assert_eq!(resolved.api_key.as_deref(), Some("sk-deepseek"));
    }

    #[test]
    fn catalog_supplies_context_window() {
        let mut home = home_with_deepseek();
        let mut models = std::collections::HashMap::new();
        models.insert(
            "deepseek-v4-flash".to_string(),
            crate::pi_agent_config::CatalogModel {
                id: "deepseek-v4-flash".into(),
                context_window: Some(1_000_000),
                max_tokens: Some(384_000),
                ..Default::default()
            },
        );
        home.catalog.insert("deepseek".into(), models);

        let resolved = resolve_model(None, Some(&home));
        assert_eq!(resolved.model.context_window, 1_000_000);
        assert_eq!(resolved.model.max_tokens, 384_000);
    }

    #[test]
    fn explicit_alias_wins() {
        let home = home_with_deepseek();
        let resolved = resolve_model(Some("claude-opus-4-7"), Some(&home));
        assert_eq!(resolved.model.provider, "anthropic");
        assert_eq!(resolved.model.id, "claude-opus-4-7");
    }

    #[test]
    fn explicit_same_provider_model() {
        let home = home_with_deepseek();
        let resolved = resolve_model(Some("deepseek-v4-pro"), Some(&home));
        assert_eq!(resolved.model.provider, "deepseek");
        assert_eq!(resolved.model.id, "deepseek-v4-pro");
        assert_eq!(resolved.api_key.as_deref(), Some("sk-deepseek"));
    }

    #[test]
    fn custom_provider_model_wins() {
        let mut home = home_with_deepseek();
        home.providers.insert(
            "empero".into(),
            CustomProvider {
                base_url: Some("https://free.example/v1".into()),
                api: Some("openai-completions".into()),
                api_key: Some("free".into()),
                models: vec![crate::pi_agent_config::CustomModel {
                    id: "glm-5.3-flash".into(),
                    context_window: Some(131_072),
                    max_tokens: Some(16_384),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        let resolved = resolve_model(Some("glm-5.3-flash"), Some(&home));
        assert_eq!(resolved.model.provider, "empero");
        assert_eq!(resolved.model.base_url, "https://free.example/v1");
        assert_eq!(resolved.model.context_window, 131_072);
        assert_eq!(resolved.api_key.as_deref(), Some("free"));
    }

    #[test]
    fn falls_back_without_home() {
        let resolved = resolve_model(Some("gpt-4o"), None);
        assert_eq!(resolved.model.id, "gpt-4o");
        assert!(resolved.api_key.is_none());
    }

    #[test]
    fn parses_thinking_levels() {
        assert_eq!(parse_thinking_level("high"), Some(ThinkingLevel::High));
        assert_eq!(parse_thinking_level("max"), Some(ThinkingLevel::Xhigh));
        assert_eq!(parse_thinking_level("nope"), None);
    }
}
