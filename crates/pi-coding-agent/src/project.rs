//! Project-prompt loading: walks up from `cwd`, gathers `AGENTS.md`,
//! `CLAUDE.md`, and `.pi/instructions.md` files, and concatenates them under
//! visible separators.
//!
//! Also resolves the system-prompt files `SYSTEM.md` (replaces the default
//! prompt) and `APPEND_SYSTEM.md` (appended to it), from `.pi/` walking up and
//! from the global agent directory.

use std::path::{Path, PathBuf};

/// Context files loaded from each ancestor directory, in order. Within a
/// directory, `AGENTS.override.md` (if present) replaces `AGENTS.md` / `CLAUDE.md`.
const CONTEXT_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];
const INSTRUCTIONS: &str = ".pi/instructions.md";

/// Load every project prompt fragment we can find, joined with separators.
/// Returns an empty string if there are no fragments.
pub fn load_project_prompt(start: &Path) -> String {
    let mut found: Vec<(PathBuf, String)> = Vec::new();
    for ancestor in start.ancestors() {
        let override_file = ancestor.join("AGENTS.override.md");
        let mut dir_files: Vec<PathBuf> = if override_file.is_file() {
            vec![override_file]
        } else {
            CONTEXT_FILES.iter().map(|n| ancestor.join(n)).collect()
        };
        dir_files.push(ancestor.join(INSTRUCTIONS));

        for path in dir_files {
            if let Some(content) = read_trimmed(&path) {
                found.push((path, content));
            }
        }
    }
    if found.is_empty() {
        return String::new();
    }
    let mut buf = String::new();
    for (path, content) in found {
        buf.push_str(&format!("\n\n----- {} -----\n", path.display()));
        buf.push_str(&content);
    }
    buf
}

/// System prompt that **replaces** the default: `.pi/SYSTEM.md` walking up,
/// else `<global_dir>/SYSTEM.md`.
pub fn load_system_override(start: &Path, global_dir: Option<&Path>) -> Option<String> {
    nearest_pi_file(start, "SYSTEM.md")
        .or_else(|| global_dir.map(|dir| dir.join("SYSTEM.md")))
        .and_then(|path| read_trimmed(&path))
}

/// Text **appended** to the system prompt: `.pi/APPEND_SYSTEM.md` walking up,
/// else `<global_dir>/APPEND_SYSTEM.md`.
pub fn load_system_append(start: &Path, global_dir: Option<&Path>) -> Option<String> {
    nearest_pi_file(start, "APPEND_SYSTEM.md")
        .or_else(|| global_dir.map(|dir| dir.join("APPEND_SYSTEM.md")))
        .and_then(|path| read_trimmed(&path))
}

fn nearest_pi_file(start: &Path, name: &str) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|ancestor| ancestor.join(".pi").join(name))
        .find(|path| path.is_file())
}

fn read_trimmed(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pi-rs-project-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn override_replaces_agents_md() {
        let dir = temp_dir("override");
        std::fs::write(dir.join("AGENTS.md"), "base agents").unwrap();
        std::fs::write(dir.join("AGENTS.override.md"), "override agents").unwrap();
        let prompt = load_project_prompt(&dir);
        assert!(prompt.contains("override agents"));
        assert!(!prompt.contains("base agents"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn system_files_prefer_project_then_global() {
        let dir = temp_dir("system");
        let pi = dir.join(".pi");
        std::fs::create_dir_all(&pi).unwrap();
        std::fs::write(pi.join("SYSTEM.md"), "project system").unwrap();
        let global = temp_dir("global");
        std::fs::write(global.join("SYSTEM.md"), "global system").unwrap();

        // Project wins.
        assert_eq!(
            load_system_override(&dir, Some(&global)).as_deref(),
            Some("project system")
        );
        // No project file → global is used.
        let empty = temp_dir("empty");
        assert_eq!(
            load_system_override(&empty, Some(&global)).as_deref(),
            Some("global system")
        );
        assert!(load_system_append(&empty, Some(&global)).is_none());
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(global).ok();
        std::fs::remove_dir_all(empty).ok();
    }
}
