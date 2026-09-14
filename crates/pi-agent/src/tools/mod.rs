//! Builtin tools: read, write, edit, bash, ls, grep, glob.
//!
//! Mirrors the core toolset shipped in `packages/coding-agent/src/core/...`.

pub mod bash;
pub mod edit;
pub mod glob_tool;
pub mod grep;
pub mod ls;
pub mod powershell;
pub mod read;
pub mod todo;
pub mod web_fetch;
pub mod write;

use std::sync::Arc;

use crate::types::AgentTool;

/// Returns the default suite of builtin tools used by the coding agent.
pub fn default_tools() -> Vec<Arc<dyn AgentTool>> {
    let mut tools: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(read::ReadTool),
        Arc::new(write::WriteTool),
        Arc::new(edit::EditTool),
        Arc::new(bash::BashTool::new()),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(glob_tool::GlobTool),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(todo::TodoTool::new()),
    ];
    if let Some(powershell) = powershell::PowerShellTool::new() {
        tools.push(Arc::new(powershell));
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        default_tools()
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    }

    #[test]
    fn includes_find_and_core_tools() {
        let names = names();
        for expected in ["read", "write", "edit", "bash", "ls", "grep", "find"] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn includes_powershell_on_windows() {
        assert!(names().contains(&"powershell".to_string()));
    }
}
