//! Default coding-agent system prompt, plus context / system-prompt file
//! loading (AGENTS.md, CLAUDE.md, .pi/instructions.md, .pi/SYSTEM.md,
//! .pi/APPEND_SYSTEM.md).

use std::path::Path;

use crate::config::AppConfig;

pub const BASE_SYSTEM_PROMPT: &str = r#"You are pi, an interactive coding assistant running in a terminal.

You have access to tools for reading and modifying files, listing directories, searching with grep and find, running shell commands via bash, fetching URLs, and tracking todos. Use them to investigate the user's repository and make focused, correct changes.

Guidelines:
- Prefer reading files before editing them; never invent code that you have not verified.
- Make small, focused diffs. Do not introduce unrelated refactors.
- After making changes, summarize what you did briefly and accurately.
- For shell-only tasks (build, test, run), use the bash tool with sensible timeouts.
- When asked an open-ended question, prefer concise answers grounded in actual files.

You operate inside the user's working directory; relative paths resolve from there.
"#;

/// Build the full system prompt.
///
/// Precedence for the base prompt: `--system-prompt` → `.pi/SYSTEM.md` /
/// global `SYSTEM.md` → [`BASE_SYSTEM_PROMPT`]. Project context files
/// (`AGENTS.md`, `CLAUDE.md`, `.pi/instructions.md`) are appended unless
/// `--no-context-files` is set, followed by `APPEND_SYSTEM.md` and any
/// `--append-system-prompt` values.
pub fn build_system_prompt(app: &AppConfig) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let global_dir = crate::pi_agent_config::agent_dir();

    let mut prompt = if let Some(custom) = &app.system_prompt {
        custom.clone()
    } else if let Some(system) = crate::project::load_system_override(&cwd, global_dir.as_deref()) {
        system
    } else {
        BASE_SYSTEM_PROMPT.to_string()
    };

    if !app.no_context_files {
        let project = crate::project::load_project_prompt(&cwd);
        if !project.is_empty() {
            prompt.push_str("\n----- project instructions -----");
            prompt.push_str(&project);
        }
    }

    if let Some(append) = crate::project::load_system_append(&cwd, global_dir.as_deref()) {
        prompt.push_str("\n\n");
        prompt.push_str(&append);
    }

    for extra in &app.system_prompt_append {
        prompt.push_str("\n\n");
        prompt.push_str(extra);
    }

    prompt
}
