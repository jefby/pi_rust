//! PowerShell tool — runs commands through `powershell -NoProfile -Command`.
//!
//! Mirrors the upstream `powershell` builtin. It is only available on Windows
//! when a PowerShell is installed; elsewhere [`PowerShellTool::new`] returns
//! `None` and the tool is not registered.

use async_trait::async_trait;
use serde_json::Value;

use crate::types::{AgentTool, AgentToolResult};

use super::bash::{BashTool, Shell};

pub struct PowerShellTool {
    inner: BashTool,
}

impl PowerShellTool {
    pub fn new() -> Option<Self> {
        Shell::powershell().map(|shell| Self {
            inner: BashTool::with_shell(shell),
        })
    }
}

#[async_trait]
impl AgentTool for PowerShellTool {
    fn name(&self) -> &str {
        "powershell"
    }
    fn requires_permission(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        "Run a PowerShell command via `powershell -NoProfile -Command <cmd>`. \
         Returns combined stdout/stderr and exit code, and `cd <path>` to change \
         persistent cwd. Available on Windows."
    }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer", "default": 120000}
            },
            "required": ["command"]
        })
    }
    async fn execute(&self, id: &str, args: Value) -> Result<AgentToolResult, String> {
        self.inner.execute(id, args).await
    }
}
