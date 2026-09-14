use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{timeout, Duration};

use crate::types::{AgentTool, AgentToolResult};

/// Which shell backs the `bash` tool.
///
/// `Cmd`/`PowerShell` are only constructed on Windows; without the allow the
/// `dead_code` lint fires on Unix even though both variants are matched on.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// `bash -lc <cmd>` (Unix, and Git Bash / MSYS2 on Windows).
    Bash,
    /// Windows `cmd.exe /C <cmd>`.
    Cmd,
    /// PowerShell `-NoProfile -Command <cmd>`.
    PowerShell,
}

/// Resolved shell used to execute commands. Detection happens once when the
/// tool is constructed so the agent keeps a stable shell for the whole run.
#[derive(Debug, Clone)]
pub(crate) struct Shell {
    program: OsString,
    kind: ShellKind,
}

impl Shell {
    /// Pick a shell for the current platform.
    ///
    /// On Unix this is always `bash -lc`. On Windows we prefer a real `bash`
    /// (Git for Windows / MSYS2 / Cygwin) so the commands the model emits keep
    /// working unchanged, then PowerShell (which shares many bash aliases such
    /// as `pwd` and `ls`), and finally the always-present `cmd.exe`.
    #[cfg(windows)]
    fn detect() -> Self {
        Self::detect_windows()
    }

    #[cfg(not(windows))]
    fn detect() -> Self {
        Self {
            program: OsString::from("bash"),
            kind: ShellKind::Bash,
        }
    }

    #[cfg(windows)]
    fn detect_windows() -> Self {
        if let Some(program) = find_bash() {
            return Self {
                program,
                kind: ShellKind::Bash,
            };
        }

        for program in ["pwsh.exe", "pwsh", "powershell.exe", "powershell"] {
            if find_in_path(program).is_some() {
                return Self {
                    program: OsString::from(program),
                    kind: ShellKind::PowerShell,
                };
            }
        }

        // `cmd.exe` ships with every Windows install.
        Self {
            program: OsString::from("cmd.exe"),
            kind: ShellKind::Cmd,
        }
    }

    /// A PowerShell shell, if one is installed. Used by the `powershell` tool;
    /// returns `None` on non-Windows hosts.
    #[cfg(windows)]
    pub(crate) fn powershell() -> Option<Self> {
        ["pwsh.exe", "pwsh", "powershell.exe", "powershell"]
            .into_iter()
            .find(|program| find_in_path(program).is_some())
            .map(|program| Self {
                program: OsString::from(program),
                kind: ShellKind::PowerShell,
            })
    }

    #[cfg(not(windows))]
    pub(crate) fn powershell() -> Option<Self> {
        None
    }

    /// Build a `Command` that runs `cmd` in this shell.
    fn command(&self, cmd: &str) -> Command {
        let mut command = Command::new(&self.program);
        match self.kind {
            ShellKind::Bash => {
                command.arg("-lc").arg(cmd);
            }
            ShellKind::Cmd => {
                command.arg("/C").arg(cmd);
            }
            ShellKind::PowerShell => {
                command.arg("-NoProfile").arg("-Command").arg(cmd);
            }
        }
        command
    }

    /// Stable name for the resolved shell (`"bash"`, `"cmd"`, `"powershell"`).
    fn name(&self) -> &'static str {
        match self.kind {
            ShellKind::Bash => "bash",
            ShellKind::Cmd => "cmd",
            ShellKind::PowerShell => "powershell",
        }
    }

    /// Human-readable invocation shown to the model in the tool description.
    fn invocation(&self) -> &'static str {
        match self.kind {
            ShellKind::Bash => "bash -lc <cmd>",
            ShellKind::Cmd => "cmd /C <cmd>",
            ShellKind::PowerShell => "powershell -NoProfile -Command <cmd>",
        }
    }
}

/// Locate `bash` on Windows: first on `PATH`, then in the usual Git for
/// Windows install directories.
#[cfg(windows)]
fn find_bash() -> Option<OsString> {
    if let Some(path) = find_in_path("bash") {
        return Some(path.into_os_string());
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(base) = std::env::var_os(var) {
            candidates.push(
                PathBuf::from(&base)
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
            );
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(&local)
                .join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe"),
        );
    }

    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(PathBuf::into_os_string)
}

/// Minimal `which` for Windows: walk `PATH`, appending `PATHEXT` suffixes.
#[cfg(windows)]
fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());

    let names: Vec<String> = if program.contains('.') {
        vec![program.to_string()]
    } else {
        let mut names = vec![program.to_string()];
        for ext in pathext.split(';').filter(|s| !s.is_empty()) {
            names.push(format!("{program}{ext}"));
        }
        names
    };

    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in &names {
            let full = dir.join(name);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

/// `std::fs::canonicalize` yields a verbatim (`\\?\`) path on Windows, which
/// prints poorly to the model and confuses some shells. Strip the prefix.
#[cfg(windows)]
fn normalize_path(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if let Some(unc) = rest.strip_prefix(r"UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        return PathBuf::from(rest);
    }
    path
}

#[cfg(not(windows))]
fn normalize_path(path: PathBuf) -> PathBuf {
    path
}

pub struct BashTool {
    cwd: Mutex<PathBuf>,
    shell: Shell,
    description: String,
}

impl BashTool {
    pub fn new() -> Self {
        Self::with_shell(Shell::detect())
    }

    /// Build a tool around an explicit shell. Used by the `powershell` tool.
    pub(crate) fn with_shell(shell: Shell) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let description = format!(
            "Run a shell command via `{}`. Returns combined stdout/stderr and exit code, and `cd <path>` to change persistent cwd.",
            shell.invocation()
        );
        Self {
            cwd: Mutex::new(normalize_path(cwd)),
            shell,
            description,
        }
    }

    /// Name of the resolved shell (`"bash"`, `"cmd"`, or `"powershell"`).
    pub fn shell_name(&self) -> &'static str {
        self.shell.name()
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn requires_permission(&self) -> bool {
        true
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout_ms": {"type": "integer", "default": 120000}
            },
            "required": ["command"]
        })
    }
    async fn execute(&self, _id: &str, args: Value) -> Result<AgentToolResult, String> {
        let cmd = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("missing 'command'")?;
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(120_000);

        let trimmed = cmd.trim();
        if let Some(rest) = trimmed.strip_prefix("cd ") {
            let target = rest.trim();
            if !target.is_empty() {
                let mut guard = self.cwd.lock().await;
                let candidate = PathBuf::from(target);
                let joined = if candidate.is_absolute() {
                    candidate
                } else {
                    guard.join(&candidate)
                };
                let resolved = normalize_path(joined.canonicalize().unwrap_or(joined));
                *guard = resolved.clone();
                return Ok(AgentToolResult::text(format!(
                    "(cwd → {})",
                    resolved.display()
                )));
            }
        }

        let cwd_snapshot = { self.cwd.lock().await.clone() };

        let mut child = self
            .shell
            .command(cmd)
            .current_dir(&cwd_snapshot)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to capture stderr".to_string())?;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let tx_out = tx.clone();
        let tx_err = tx.clone();
        drop(tx);

        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx_out.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx_err.send(format!("[stderr] {line}")).is_err() {
                    break;
                }
            }
        });

        let combined = Arc::new(Mutex::new(String::new()));
        let combined_collector = combined.clone();
        let collector = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let mut buf = combined_collector.lock().await;
                if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str(&line);
            }
        });

        let status = match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
            Ok(Ok(s)) => {
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                let _ = collector.await;
                s
            }
            Ok(Err(e)) => {
                let _ = child.kill().await;
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                let _ = collector.await;
                return Err(format!("wait: {e}"));
            }
            Err(_) => {
                let _ = child.kill().await;
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                let _ = collector.await;
                return Err(format!("command timed out after {timeout_ms}ms"));
            }
        };

        let code = status.code().unwrap_or(-1);
        let mut out = combined.lock().await.clone();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("[exit {code}]"));
        Ok(AgentToolResult::text(out))
    }
}
