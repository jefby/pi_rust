//! Optional on-disk defaults for the `pi` CLI.
//!
//! Loads `$XDG_CONFIG_HOME/pi/config.toml` (resolved via [`dirs::config_dir`]
//! joined with `pi`). The file is **entirely optional**: if it is missing,
//! unreadable, or malformed TOML, [`load`] silently falls back to
//! [`FileConfig::default`] so the CLI keeps starting.
//!
//! All keys are optional. CLI flags and environment variables take
//! precedence over the file; the file only fills holes.
//!
//! # Schema
//!
//! ```toml
//! # ~/.config/pi/config.toml
//!
//! # Model id (e.g. "claude-sonnet-4-6", "gpt-4o-mini"). Used to seed
//! # PI_MODEL when that env var is unset.
//! model = "claude-sonnet-4-6"
//!
//! # Maximum agent turns before the loop stops.
//! max_turns = 32
//!
//! # Extended-thinking budget bucket. One of:
//! # "off" | "minimal" | "low" | "medium" | "high" | "xhigh".
//! thinking_level = "off"
//!
//! # Default for the `--yolo` flag (skips permission prompts).
//! yolo = false
//!
//! # Default for the `--json` flag (JSON-lines in print mode).
//! json = false
//!
//! # Summarize the abandoned branch when `/tree` switches branches.
//! branch_summary = true
//! ```

use std::path::PathBuf;

use serde::Deserialize;

/// Parsed contents of `$XDG_CONFIG_HOME/pi/config.toml`. All fields are
/// optional; missing keys deserialize to `None` / `false`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    /// Model identifier. Used to seed `PI_MODEL` if that env var is unset.
    pub model: Option<String>,
    /// Maximum agent turns before stopping.
    pub max_turns: Option<u32>,
    /// Extended-thinking budget bucket ("off"/"minimal"/"low"/"medium"/"high"/"xhigh").
    pub thinking_level: Option<String>,
    /// Default for `--yolo` (skip permission prompts).
    #[serde(default)]
    pub yolo: bool,
    /// Default for `--json` (JSON-lines in print mode).
    #[serde(default)]
    pub json: bool,
    /// Whether `/tree` branch switches summarize the abandoned branch.
    pub branch_summary: Option<bool>,
}

/// Path to the config file under the platform-appropriate config dir.
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("pi").join("config.toml"))
}

/// Load [`FileConfig`] from `$XDG_CONFIG_HOME/pi/config.toml`.
///
/// Returns [`FileConfig::default`] if the file is missing or cannot be
/// parsed. Errors are swallowed by design — a broken config file must not
/// prevent the CLI from starting.
pub fn load() -> FileConfig {
    let Some(path) = config_path() else {
        return FileConfig::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return FileConfig::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty() {
        let c: FileConfig = toml::from_str("").unwrap();
        assert!(c.model.is_none());
        assert_eq!(c.max_turns, None);
        assert!(c.thinking_level.is_none());
        assert!(!c.yolo);
        assert!(!c.json);
    }

    #[test]
    fn parses_all_fields() {
        let src = r#"
            model = "claude-opus-4-7"
            max_turns = 64
            thinking_level = "medium"
            yolo = true
            json = true
        "#;
        let c: FileConfig = toml::from_str(src).unwrap();
        assert_eq!(c.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(c.max_turns, Some(64));
        assert_eq!(c.thinking_level.as_deref(), Some("medium"));
        assert!(c.yolo);
        assert!(c.json);
    }

    #[test]
    fn rejects_unknown_keys() {
        let src = r#"
            bogus = true
        "#;
        let r: Result<FileConfig, _> = toml::from_str(src);
        assert!(r.is_err());
    }
}
