//! Interactive permission prompt for the CLI.

use std::io::{BufRead, Write};
use std::sync::Mutex;

use async_trait::async_trait;
use pi_agent::{PermissionDecision, PermissionPolicy};
use serde_json::Value;

#[allow(dead_code)] // DenyAll is a public mode, kept for sandboxed runs.
pub enum Mode {
    /// Always allow without asking.
    Yolo,
    /// Prompt the user interactively (stdin / stderr).
    Interactive,
    /// Always deny — useful for sandboxed runs.
    DenyAll,
}

pub struct CliPermission {
    mode: Mode,
    allowed_session: Mutex<std::collections::HashSet<String>>,
}

impl CliPermission {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            allowed_session: Mutex::new(Default::default()),
        }
    }
}

#[async_trait]
impl PermissionPolicy for CliPermission {
    async fn check(&self, tool_name: &str, args: &Value) -> PermissionDecision {
        if let Ok(set) = self.allowed_session.lock() {
            if set.contains(tool_name) {
                return PermissionDecision::Allow;
            }
        }
        match self.mode {
            Mode::Yolo => PermissionDecision::Allow,
            Mode::DenyAll => PermissionDecision::Deny {
                reason: "permissions disabled".into(),
            },
            Mode::Interactive => prompt(tool_name, args, &self.allowed_session).await,
        }
    }
}

async fn prompt(
    tool_name: &str,
    args: &Value,
    allowed_session: &Mutex<std::collections::HashSet<String>>,
) -> PermissionDecision {
    let tool_name = tool_name.to_string();
    let args_pretty = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
    let allowed_session_clone: Mutex<std::collections::HashSet<String>> = Mutex::new(
        allowed_session
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default(),
    );
    // run the blocking prompt off the runtime thread
    let decision = tokio::task::spawn_blocking(move || {
        let mut err = std::io::stderr();
        let _ = writeln!(err, "\n⚠ tool call requires permission: {tool_name}");
        let _ = writeln!(err, "{args_pretty}");
        let _ = write!(err, "Allow? [y]es / [a]llow-session / [n]o: ");
        let _ = err.flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        let ans = line.trim().to_lowercase();
        match ans.as_str() {
            "y" | "yes" | "" => PermissionDecision::Allow,
            "a" | "all" | "allow" | "session" => PermissionDecision::AllowSession,
            _ => PermissionDecision::Deny {
                reason: "user denied".into(),
            },
        }
    })
    .await
    .unwrap_or(PermissionDecision::Deny {
        reason: "permission prompt failed".into(),
    });

    if decision == PermissionDecision::AllowSession {
        if let Ok(mut s) = allowed_session.lock() {
            // Suffix would have been pre-cloned in allowed_session_clone; merge.
            if let Ok(snap) = allowed_session_clone.lock() {
                s.extend(snap.iter().cloned());
            }
        }
    }
    decision
}

/// Permission plumbing for the full-screen TUI. The policy forwards each
/// request to the UI event loop and awaits the answer, instead of reading
/// stdin (which the TUI owns).
#[cfg(feature = "tui")]
pub mod tui {
    use std::sync::Arc;

    use async_trait::async_trait;
    use pi_agent::{PermissionDecision, PermissionPolicy};
    use serde_json::Value;
    use tokio::sync::{mpsc, oneshot};

    /// A permission request routed to the TUI.
    pub struct PermissionRequest {
        pub tool_name: String,
        pub args: Value,
        pub respond: oneshot::Sender<PermissionDecision>,
    }

    /// [`PermissionPolicy`] that asks the TUI instead of stdin.
    pub struct TuiPermission {
        tx: mpsc::UnboundedSender<PermissionRequest>,
    }

    impl TuiPermission {
        /// Returns the shared policy plus the receiver the TUI must drain.
        #[allow(clippy::new_ret_no_self)]
        pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<PermissionRequest>) {
            let (tx, rx) = mpsc::unbounded_channel();
            (Arc::new(Self { tx }), rx)
        }
    }

    #[async_trait]
    impl PermissionPolicy for TuiPermission {
        async fn check(&self, tool_name: &str, args: &Value) -> PermissionDecision {
            let (respond, rx) = oneshot::channel();
            let request = PermissionRequest {
                tool_name: tool_name.to_string(),
                args: args.clone(),
                respond,
            };
            if self.tx.send(request).is_err() {
                return PermissionDecision::Deny {
                    reason: "UI is not available".into(),
                };
            }
            rx.await.unwrap_or(PermissionDecision::Deny {
                reason: "UI closed".into(),
            })
        }
    }
}
