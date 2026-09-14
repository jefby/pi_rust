//! Full-screen terminal UI, enabled with the `tui` cargo feature.
//!
//! Layout: a title bar, a scrollable conversation pane, an input box, and a
//! status line. Agent events stream into the conversation; tool runs show up
//! inline; permission prompts render as a modal answered with `y` / `a` / `n`.
//!
//! Fall back to the line REPL when the feature is off or the terminal is not a
//! TTY.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use pi_agent::{
    run_agent_with_history, tools::default_tools, AgentConfig, AgentEvent, AgentRun,
    PermissionDecision,
};
use pi_ai::{Content, Message};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::AppConfig;
use crate::permission::tui::{PermissionRequest, TuiPermission};
use crate::session::Session;
use crate::slash;
use crate::system_prompt::build_system_prompt;

/// Messages the background agent tasks send back to the UI.
enum UiEvent {
    Agent(AgentEvent),
    /// The turn finished (or failed); carries the final run for persistence.
    TurnDone(Box<pi_agent::Result<AgentRun>>),
    /// `/compact` finished; carries the rewritten session and its status.
    CompactDone {
        session: Box<Session>,
        result: Result<String, String>,
    },
    /// A branch summary finished generating.
    BranchSummary(Result<String, String>),
}

struct PendingPermission {
    tool_name: String,
    args: String,
    respond: oneshot::Sender<PermissionDecision>,
}

/// Session picker shown for a bare `-r`.
struct Picker {
    sessions: Vec<crate::session::SessionSummary>,
    selected: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeMode {
    /// Switch the active leaf to the selected entry (`/tree`).
    Switch,
    /// Start a new session from the selected user message (`/fork`).
    Fork,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeFilter {
    Default,
    NoTools,
    UserOnly,
    All,
}

impl TreeFilter {
    fn next(self) -> Self {
        match self {
            TreeFilter::Default => TreeFilter::NoTools,
            TreeFilter::NoTools => TreeFilter::UserOnly,
            TreeFilter::UserOnly => TreeFilter::All,
            TreeFilter::All => TreeFilter::Default,
        }
    }

    fn label(self) -> &'static str {
        match self {
            TreeFilter::Default => "default",
            TreeFilter::NoTools => "no-tools",
            TreeFilter::UserOnly => "user-only",
            TreeFilter::All => "all",
        }
    }

    fn keep(self, kind: crate::session::EntryKind) -> bool {
        use crate::session::EntryKind;
        match self {
            TreeFilter::All => true,
            TreeFilter::Default => kind != EntryKind::ToolResult,
            TreeFilter::NoTools => {
                !matches!(kind, EntryKind::ToolResult | EntryKind::AssistantToolOnly)
            }
            TreeFilter::UserOnly => kind == EntryKind::User,
        }
    }
}

/// Tree overlay with fold/unfold and filtering.
struct TreeOverlay {
    /// Every entry in depth-first (tree) order.
    items: Vec<crate::session::TreeItem>,
    /// Index into `items`.
    selected: usize,
    collapsed: std::collections::HashSet<String>,
    filter: TreeFilter,
    mode: TreeMode,
}

/// A flattened tree row with upstream-style indentation and connectors.
struct FlatRow {
    item: usize,
    display_indent: usize,
    show_connector: bool,
    is_last: bool,
    gutters: Vec<(usize, bool)>,
    is_virtual_root_child: bool,
    foldable: bool,
    folded: bool,
    on_active_path: bool,
}

/// Build render rows the way upstream `pi`'s tree selector does: only branch
/// points indent and draw connectors; single-child chains stay flat. Filtered
/// nodes are skipped but their children are promoted; folded nodes hide their
/// whole subtree. Subtrees containing the active leaf are ordered first.
fn build_tree_rows(tree: &TreeOverlay, active_leaf: Option<&str>) -> Vec<FlatRow> {
    use std::collections::{HashMap, HashSet};

    let items = &tree.items;
    let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item.parent_id.as_deref().filter(|p| ids.contains(p)) {
            Some(parent) => children.entry(parent).or_default().push(i),
            None => roots.push(i),
        }
    }

    // Subtrees that contain the active leaf are ordered first.
    let mut contains_active: HashMap<usize, bool> = HashMap::new();
    let mut pre = Vec::new();
    {
        let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            pre.push(i);
            if let Some(kids) = children.get(items[i].id.as_str()) {
                for &k in kids.iter().rev() {
                    stack.push(k);
                }
            }
        }
    }
    for &i in pre.iter().rev() {
        let mut has = active_leaf == Some(items[i].id.as_str());
        if let Some(kids) = children.get(items[i].id.as_str()) {
            for &k in kids {
                if contains_active.get(&k).copied().unwrap_or(false) {
                    has = true;
                }
            }
        }
        contains_active.insert(i, has);
    }

    let by_id: HashMap<&str, usize> = items
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.as_str(), i))
        .collect();
    let mut active_path: HashSet<String> = HashSet::new();
    let mut cursor = active_leaf.map(str::to_string);
    while let Some(id) = cursor {
        if !active_path.insert(id.clone()) {
            break;
        }
        cursor = by_id
            .get(id.as_str())
            .and_then(|&i| items[i].parent_id.clone());
    }

    fn visible_children(
        i: usize,
        items: &[crate::session::TreeItem],
        children: &HashMap<&str, Vec<usize>>,
        tree: &TreeOverlay,
        contains_active: &HashMap<usize, bool>,
        out: &mut Vec<usize>,
    ) {
        if let Some(kids) = children.get(items[i].id.as_str()) {
            let mut ordered = kids.clone();
            ordered.sort_by_key(|&k| !contains_active.get(&k).copied().unwrap_or(false));
            for k in ordered {
                if tree.collapsed.contains(&items[k].id) {
                    continue;
                }
                if tree.filter.keep(items[k].kind) {
                    out.push(k);
                } else {
                    visible_children(k, items, children, tree, contains_active, out);
                }
            }
        }
    }

    let mut vis_roots: Vec<usize> = Vec::new();
    for &r in &roots {
        if tree.filter.keep(items[r].kind) {
            vis_roots.push(r);
        } else {
            visible_children(r, items, &children, tree, &contains_active, &mut vis_roots);
        }
    }
    vis_roots.sort_by_key(|&r| !contains_active.get(&r).copied().unwrap_or(false));
    let multiple_roots = vis_roots.len() > 1;

    let mut out: Vec<FlatRow> = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut stack: Vec<(usize, usize, bool, bool, bool, Vec<(usize, bool)>, bool)> = Vec::new();
    let root_count = vis_roots.len();
    for idx in (0..root_count).rev() {
        stack.push((
            vis_roots[idx],
            if multiple_roots { 1 } else { 0 },
            multiple_roots,
            multiple_roots,
            idx + 1 == root_count,
            Vec::new(),
            multiple_roots,
        ));
    }

    while let Some((
        i,
        indent,
        just_branched,
        show_connector,
        is_last,
        gutters,
        is_virtual_root_child,
    )) = stack.pop()
    {
        let item = &items[i];
        let foldable = children
            .get(item.id.as_str())
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        let folded = tree.collapsed.contains(&item.id);
        out.push(FlatRow {
            item: i,
            display_indent: if multiple_roots {
                indent.saturating_sub(1)
            } else {
                indent
            },
            show_connector,
            is_last,
            gutters: gutters.clone(),
            is_virtual_root_child,
            foldable,
            folded,
            on_active_path: active_path.contains(&item.id),
        });

        if folded {
            continue;
        }

        let mut kids = Vec::new();
        visible_children(i, items, &children, tree, &contains_active, &mut kids);
        let multiple_children = kids.len() > 1;
        let child_indent = if multiple_children || (just_branched && indent > 0) {
            indent + 1
        } else {
            indent
        };
        let connector_displayed = show_connector && !is_virtual_root_child;
        let connector_position = if multiple_roots {
            indent.saturating_sub(1).saturating_sub(1)
        } else {
            indent.saturating_sub(1)
        };
        let mut child_gutters = gutters;
        if connector_displayed {
            child_gutters.push((connector_position, !is_last));
        }
        let count = kids.len();
        for (k, &child) in kids.iter().enumerate().rev() {
            stack.push((
                child,
                child_indent,
                multiple_children,
                multiple_children,
                k + 1 == count,
                child_gutters.clone(),
                false,
            ));
        }
    }
    out
}

struct State {
    app: AppConfig,
    permission: Arc<TuiPermission>,
    session: Session,
    system_prompt: String,
    lines: Vec<Line<'static>>,
    streaming: String,
    thinking: String,
    input: String,
    cursor: usize,
    follow: bool,
    scroll: usize,
    max_scroll: usize,
    busy: bool,
    spinner: usize,
    quit: bool,
    pending_perm: Option<PendingPermission>,
    picker: Option<Picker>,
    tree: Option<TreeOverlay>,
    /// Cached `git rev-parse --abbrev-ref HEAD` for the footer.
    git_branch: Option<String>,
    /// Length of the active branch when the current turn started; new messages
    /// from the turn are appended from this index.
    branch_len: usize,
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Maximum number of text rows the input box grows to before it scrolls.
const MAX_INPUT_ROWS: usize = 6;

/// Run the TUI until the user quits.
pub async fn run_tui(
    app: &AppConfig,
    permission: Arc<TuiPermission>,
    permission_rx: mpsc::UnboundedReceiver<PermissionRequest>,
    initial: Option<Session>,
    pick: bool,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, app, permission, permission_rx, initial, pick).await;
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut DefaultTerminal,
    app: &AppConfig,
    permission: Arc<TuiPermission>,
    mut permission_rx: mpsc::UnboundedReceiver<PermissionRequest>,
    initial: Option<Session>,
    pick: bool,
) -> anyhow::Result<()> {
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiEvent>();

    let session = initial.unwrap_or_else(|| Session::new(&app.model));
    let mut state = State {
        app: app.clone(),
        permission,
        session,
        system_prompt: build_system_prompt(app),
        lines: Vec::new(),
        streaming: String::new(),
        thinking: String::new(),
        input: String::new(),
        cursor: 0,
        follow: true,
        scroll: 0,
        max_scroll: 0,
        busy: false,
        spinner: 0,
        quit: false,
        pending_perm: None,
        picker: None,
        tree: None,
        git_branch: detect_git_branch(),
        branch_len: 0,
    };
    if pick {
        // Bare `-r`: start empty and let the user choose a session.
        state.session = Session::new(&app.model);
        state.open_picker();
    } else if !state.session.is_empty() {
        state.replay_history();
        state.push_system(format!(
            "(resumed session {}, {} prior messages)",
            state.session.id,
            state.session.messages().len()
        ));
    }
    state.push_system(format!(
        "model: {} ({}) — /help for commands",
        state.app.model.name, state.app.model.provider
    ));

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(120));

    while !state.quit {
        terminal.draw(|frame| state.render(frame))?;

        tokio::select! {
            maybe = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe {
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        state.on_key(key, &ui_tx);
                    }
                }
            }
            Some(ui_event) = ui_rx.recv() => {
                match ui_event {
                    UiEvent::Agent(ev) => state.on_agent_event(ev),
                    UiEvent::TurnDone(res) => state.finish_turn(*res),
                    UiEvent::CompactDone { session, result } => {
                        state.session = *session;
                        match result {
                            Ok(msg) => state.push_system(msg),
                            Err(e) => state.push_error(format!("compact failed: {e}")),
                        }
                        state.save();
                    }
                    UiEvent::BranchSummary(result) => state.finish_branch_summary(result),
                }
            }
            Some(req) = permission_rx.recv() => {
                state.pending_perm = Some(PendingPermission {
                    tool_name: req.tool_name,
                    args: serde_json::to_string_pretty(&req.args).unwrap_or_default(),
                    respond: req.respond,
                });
            }
            _ = ticker.tick() => {
                state.spinner = state.spinner.wrapping_add(1);
            }
        }
    }
    Ok(())
}

impl State {
    fn on_key(&mut self, key: KeyEvent, ui_tx: &mpsc::UnboundedSender<UiEvent>) {
        // Ctrl+C saves and quits; if a turn is in flight, record what streamed.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.flush_thinking();
            if self.busy {
                self.record_partial_assistant();
            }
            self.save();
            self.quit = true;
            return;
        }

        // An open session picker owns the keyboard.
        if self.picker.is_some() {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.picker_move(-1),
                KeyCode::Down | KeyCode::Char('j') => self.picker_move(1),
                KeyCode::Enter => self.picker_confirm(),
                KeyCode::Esc => self.picker = None,
                _ => {}
            }
            return;
        }

        // An open tree overlay owns the keyboard.
        if self.tree.is_some() {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.tree_move(-1),
                KeyCode::Down | KeyCode::Char('j') => self.tree_move(1),
                KeyCode::Left if ctrl || alt => self.tree_collapse(),
                KeyCode::Right if ctrl || alt => self.tree_expand(),
                KeyCode::Left => self.tree_move(-10),
                KeyCode::Right => self.tree_move(10),
                KeyCode::PageUp => self.tree_move(-10),
                KeyCode::PageDown => self.tree_move(10),
                KeyCode::Char('o') if ctrl => self.tree_filter_next(),
                KeyCode::Enter => self.tree_confirm(ui_tx),
                KeyCode::Esc => self.tree = None,
                _ => {}
            }
            return;
        }

        // A pending permission prompt owns the keyboard.
        if self.pending_perm.is_some() {
            let decision = match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => Some(PermissionDecision::Allow),
                KeyCode::Char('a') | KeyCode::Char('A') => Some(PermissionDecision::AllowSession),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    Some(PermissionDecision::Deny {
                        reason: "user denied".into(),
                    })
                }
                _ => None,
            };
            if let Some(decision) = decision {
                if let Some(pending) = self.pending_perm.take() {
                    let _ = pending.respond.send(decision);
                }
            }
            return;
        }

        match key.code {
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    let idx = self.byte_index();
                    self.input.insert(idx, '\n');
                    self.cursor += 1;
                } else if self.busy {
                    self.push_system("(still working…)".to_string());
                } else {
                    self.start_submit(ui_tx);
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.input.is_empty() {
                    self.quit = true;
                }
            }
            KeyCode::Char(c) => {
                let idx = self.byte_index();
                self.input.insert(idx, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let idx = self.byte_index();
                    if let Some(prev) = self.input[..idx].chars().next_back() {
                        let start = idx - prev.len_utf8();
                        self.input.remove(start);
                        self.cursor -= 1;
                    }
                }
            }
            KeyCode::Delete => {
                let idx = self.byte_index();
                if idx < self.input.len() {
                    self.input.remove(idx);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.input.chars().count());
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Up => self.scroll_up(1),
            KeyCode::Down => self.scroll_down(1),
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            _ => {}
        }
    }

    fn start_submit(&mut self, ui_tx: &mpsc::UnboundedSender<UiEvent>) {
        let prompt = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;
        if prompt.is_empty() {
            return;
        }

        if prompt.starts_with('/') {
            self.run_slash(prompt, ui_tx);
            return;
        }

        self.push_user(prompt.clone());
        self.streaming.clear();
        self.follow = true;
        self.busy = true;

        // Persist the user message *before* the turn runs so an interrupt
        // (Ctrl+C) or crash still leaves a resumable session.
        self.session.push_message(Message::user_text(prompt));
        self.save();

        let cfg = AgentConfig::new(self.app.model.clone(), self.system_prompt.clone())
            .with_tools(default_tools())
            .with_max_turns(self.app.max_turns)
            .with_thinking(self.app.thinking_level)
            .with_api_key(self.app.api_key.clone())
            .with_permission(self.permission.clone());
        let history = self.session.messages();
        self.branch_len = history.len();

        let ui = ui_tx.clone();
        tokio::spawn(async move {
            let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();
            let ui_forward = ui.clone();
            let forward = tokio::spawn(async move {
                while let Some(ev) = agent_rx.recv().await {
                    if ui_forward.send(UiEvent::Agent(ev)).is_err() {
                        break;
                    }
                }
            });
            let res = run_agent_with_history(&cfg, history, Some(agent_tx)).await;
            let _ = forward.await;
            let _ = ui.send(UiEvent::TurnDone(Box::new(res)));
        });
    }

    fn run_slash(&mut self, prompt: String, ui_tx: &mpsc::UnboundedSender<UiEvent>) {
        if prompt == "/compact" || prompt.starts_with("/compact ") {
            self.push_system("(compacting…)".to_string());
            let app = self.app.clone();
            let session = self.session.clone();
            let ui = ui_tx.clone();
            tokio::spawn(async move {
                let mut session = session;
                let result = slash::compact(&app, &mut session)
                    .await
                    .map_err(|e| e.to_string());
                let _ = ui.send(UiEvent::CompactDone {
                    session: Box::new(session),
                    result,
                });
            });
            return;
        }

        match slash::handle(&prompt, &self.app, &mut self.session) {
            Ok(outcome) => {
                for l in outcome.output {
                    self.push_system(l);
                }
                match outcome.action {
                    slash::SlashAction::Tree => self.open_tree(TreeMode::Switch),
                    slash::SlashAction::Fork => self.open_tree(TreeMode::Fork),
                    slash::SlashAction::New => {
                        // Clear the view for the fresh session.
                        self.lines.clear();
                        self.streaming.clear();
                        self.thinking.clear();
                        self.input.clear();
                        self.cursor = 0;
                        self.push_system(format!(
                            "model: {} ({}) — /help for commands",
                            self.app.model.name, self.app.model.provider
                        ));
                    }
                    slash::SlashAction::None => {}
                }
                if !outcome.keep_going {
                    self.quit = true;
                }
                self.save();
            }
            Err(e) => self.push_error(format!("{e}")),
        }
    }

    fn on_agent_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::TextDelta { delta } => {
                self.streaming.push_str(&delta);
                self.follow = true;
            }
            AgentEvent::ThinkingDelta { delta } => {
                self.thinking.push_str(&delta);
                self.follow = true;
            }
            AgentEvent::AssistantMessage { message } => {
                self.flush_thinking();
                self.streaming.clear();
                if let Message::Assistant(a) = &message {
                    let mut text = String::new();
                    for c in &a.content {
                        if let Content::Text { text: t } = c {
                            text.push_str(t);
                        }
                    }
                    if !text.is_empty() {
                        self.push_assistant(text);
                    }
                }
            }
            AgentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                let pretty = serde_json::to_string(&args).unwrap_or_default();
                self.push_tool(format!("→ {} {}", tool_name, slash::truncate(&pretty, 160)));
            }
            AgentEvent::ToolExecutionEnd {
                tool_name,
                is_error,
                ..
            } => {
                let status = if is_error { "error" } else { "ok" };
                self.push_tool(format!("← {tool_name} {status}"));
            }
            AgentEvent::PermissionDenied { tool_name, reason } => {
                self.push_error(format!("✗ {tool_name} denied: {reason}"));
            }
            _ => {}
        }
    }

    fn finish_turn(&mut self, res: pi_agent::Result<AgentRun>) {
        match res {
            Ok(run) => {
                if let Some(new_messages) = run.messages.get(self.branch_len..) {
                    self.session.append_messages(new_messages);
                }
            }
            Err(e) => {
                self.push_error(format!("agent error: {e}"));
                self.record_partial_assistant();
            }
        }
        if !self.streaming.is_empty() {
            let text = std::mem::take(&mut self.streaming);
            self.push_assistant(text);
        }
        self.flush_thinking();
        self.save();
        self.busy = false;
    }

    /// Persist whatever assistant text streamed so far as an aborted assistant
    /// message, keeping the transcript alternating and resumable.
    fn record_partial_assistant(&mut self) {
        if self.streaming.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.streaming);
        let message = Message::Assistant(pi_ai::AssistantMessage {
            content: vec![Content::text(text.clone())],
            api: self.app.model.api.clone(),
            provider: self.app.model.provider.clone(),
            model: self.app.model.id.clone(),
            usage: pi_ai::Usage::default(),
            stop_reason: pi_ai::StopReason::Aborted,
            error_message: None,
            timestamp: pi_ai::now_ms(),
        });
        self.session.push_message(message);
        self.push_assistant(text);
    }

    fn save(&mut self) {
        if let Err(e) = crate::session::save(&self.app.config_dir, &mut self.session) {
            self.push_system(format!("(warning: session save failed: {e})"));
        }
    }

    /// Load the session list and show the picker (bare `-r`).
    fn open_picker(&mut self) {
        match crate::session::list(&self.app.config_dir) {
            Ok(sessions) if !sessions.is_empty() => {
                self.picker = Some(Picker {
                    sessions,
                    selected: 0,
                });
            }
            Ok(_) => self.push_system("(no saved sessions)".to_string()),
            Err(e) => self.push_error(format!("failed to list sessions: {e}")),
        }
    }

    fn picker_move(&mut self, delta: isize) {
        if let Some(picker) = &mut self.picker {
            let len = picker.sessions.len() as isize;
            if len > 0 {
                picker.selected = (picker.selected as isize + delta).rem_euclid(len) as usize;
            }
        }
    }

    fn picker_confirm(&mut self) {
        let Some(picker) = self.picker.take() else {
            return;
        };
        let Some(id) = picker.sessions.get(picker.selected).map(|s| s.id.clone()) else {
            return;
        };
        match crate::session::load(&self.app.config_dir, &id) {
            Ok(session) => {
                self.session = session;
                self.lines.clear();
                self.replay_history();
                self.push_system(format!(
                    "(resumed session {}, {} prior messages)",
                    self.session.id,
                    self.session.messages().len()
                ));
            }
            Err(e) => {
                self.push_error(format!("load failed: {e}"));
                self.picker = Some(picker);
            }
        }
    }

    fn open_tree(&mut self, mode: TreeMode) {
        let items = self.session.tree_items();
        if items.is_empty() {
            self.push_system("(no entries to choose from)".to_string());
            return;
        }
        let filter = if mode == TreeMode::Fork {
            TreeFilter::UserOnly
        } else {
            TreeFilter::Default
        };
        let mut overlay = TreeOverlay {
            items,
            selected: 0,
            collapsed: Default::default(),
            filter,
            mode,
        };
        // Start on the active leaf (or the first visible row).
        let rows = build_tree_rows(&overlay, self.session.active_leaf.as_deref());
        if let Some(leaf) = self.session.active_leaf.as_deref() {
            if let Some(row) = rows.iter().find(|r| overlay.items[r.item].id == leaf) {
                overlay.selected = row.item;
            }
        }
        if overlay.selected == 0 {
            if let Some(row) = rows.first() {
                overlay.selected = row.item;
            }
        }
        self.tree = Some(overlay);
    }

    fn tree_move(&mut self, delta: isize) {
        let Some(tree) = &self.tree else {
            return;
        };
        let rows = build_tree_rows(tree, self.session.active_leaf.as_deref());
        if rows.is_empty() {
            return;
        }
        let position = rows
            .iter()
            .position(|r| r.item == tree.selected)
            .unwrap_or(0);
        let next = (position as isize + delta).rem_euclid(rows.len() as isize) as usize;
        let target = rows[next].item;
        if let Some(tree) = &mut self.tree {
            tree.selected = target;
        }
    }

    /// Collapse the selected subtree, or move to its parent when it is a leaf.
    fn tree_collapse(&mut self) {
        let Some(tree) = &mut self.tree else {
            return;
        };
        let item = &tree.items[tree.selected];
        let foldable = tree
            .items
            .iter()
            .any(|it| it.parent_id.as_deref() == Some(item.id.as_str()));
        if foldable {
            let id = item.id.clone();
            tree.collapsed.insert(id);
        } else if let Some(parent) = item.parent_id.clone() {
            if let Some(idx) = tree.items.iter().position(|it| it.id == parent) {
                tree.selected = idx;
            }
        }
    }

    /// Expand the selected node, or move to its first child when already open.
    fn tree_expand(&mut self) {
        let Some(tree) = &mut self.tree else {
            return;
        };
        let id = tree.items[tree.selected].id.clone();
        if tree.collapsed.remove(&id) {
            return;
        }
        if let Some(child) = tree
            .items
            .iter()
            .position(|it| it.parent_id.as_deref() == Some(id.as_str()))
        {
            tree.selected = child;
        }
    }

    fn tree_filter_next(&mut self) {
        if let Some(tree) = &mut self.tree {
            tree.filter = tree.filter.next();
        }
        self.tree_reselect_visible();
    }

    /// Keep the selection on a visible row after filtering/folding changes.
    fn tree_reselect_visible(&mut self) {
        let Some(tree) = &self.tree else {
            return;
        };
        let rows = build_tree_rows(tree, self.session.active_leaf.as_deref());
        if rows.iter().any(|r| r.item == tree.selected) {
            return;
        }
        let items = &tree.items;
        let mut cursor = items[tree.selected].parent_id.clone();
        while let Some(id) = cursor {
            if let Some(row) = rows.iter().find(|r| items[r.item].id == id) {
                let target = row.item;
                if let Some(tree) = &mut self.tree {
                    tree.selected = target;
                }
                return;
            }
            cursor = items
                .iter()
                .find(|it| it.id == id)
                .and_then(|it| it.parent_id.clone());
        }
        if let Some(row) = rows.first() {
            let target = row.item;
            if let Some(tree) = &mut self.tree {
                tree.selected = target;
            }
        }
    }

    fn tree_confirm(&mut self, ui_tx: &mpsc::UnboundedSender<UiEvent>) {
        let Some(tree) = self.tree.take() else {
            return;
        };
        let selected = &tree.items[tree.selected];
        let id = selected.id.clone();
        let kind = selected.kind;
        let parent = selected.parent_id.clone();

        match tree.mode {
            TreeMode::Fork => {
                let prompt = self.user_text(&id);
                let prefix: Vec<Message> = parent
                    .as_deref()
                    .map(|p| {
                        self.session
                            .branch_to(p)
                            .into_iter()
                            .filter_map(|e| e.message.clone())
                            .collect()
                    })
                    .unwrap_or_default();

                let mut new = Session::new(&self.app.model);
                new.replace_messages(prefix);
                self.session = new;
                self.lines.clear();
                self.replay_history();
                self.input = prompt;
                self.cursor = self.input.chars().count();
                self.push_system(format!("(forked into new session {})", self.session.id));
                self.save();
            }
            TreeMode::Switch => {
                use crate::session::EntryKind;
                // Selecting a user message moves the leaf to its *parent* and
                // puts the message back in the editor (edit & resubmit creates
                // a new branch). Any other entry becomes the new leaf.
                let (new_leaf, prompt) = if kind == EntryKind::User {
                    (parent.clone(), self.user_text(&id))
                } else {
                    (Some(id.clone()), String::new())
                };
                let old_leaf = self.session.active_leaf.clone();
                let abandoned = self
                    .session
                    .abandoned(old_leaf.as_deref(), new_leaf.as_deref());
                self.session.set_active_leaf(new_leaf);
                self.lines.clear();
                self.replay_history();
                self.input = prompt;
                self.cursor = self.input.chars().count();
                self.push_system(format!(
                    "(switched to {} messages)",
                    self.session.messages().len()
                ));
                self.save();
                self.summarize_abandoned(abandoned, ui_tx);
            }
        }
    }

    /// Text of a user entry, if it is one.
    fn user_text(&self, id: &str) -> String {
        self.session
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| match &e.message {
                Some(Message::User { content, .. }) => content
                    .iter()
                    .filter_map(|c| c.as_text())
                    .collect::<Vec<_>>()
                    .join(""),
                _ => String::new(),
            })
            .unwrap_or_default()
    }

    fn summarize_abandoned(
        &mut self,
        abandoned: Vec<Message>,
        ui_tx: &mpsc::UnboundedSender<UiEvent>,
    ) {
        if abandoned.is_empty() {
            return;
        }
        if !self.app.summarize_branches {
            self.push_system(format!(
                "({} abandoned message(s); branch summary disabled)",
                abandoned.len()
            ));
            return;
        }
        self.push_system("(summarizing abandoned branch…)".to_string());
        let app = self.app.clone();
        let ui = ui_tx.clone();
        tokio::spawn(async move {
            let result = slash::summarize(&app, abandoned)
                .await
                .map_err(|e| e.to_string());
            let _ = ui.send(UiEvent::BranchSummary(result));
        });
    }

    fn finish_branch_summary(&mut self, result: Result<String, String>) {
        match result {
            Ok(summary) if !summary.trim().is_empty() => {
                let text = format!("[branch summary]\n{}", summary.trim());
                self.session.push_message(Message::user_text(text.clone()));
                self.push_system("(branch summary)".to_string());
                for line in text.split('\n') {
                    self.push_system(line.to_string());
                }
                self.save();
            }
            Ok(_) => {}
            Err(e) => self.push_error(format!("branch summary failed: {e}")),
        }
    }

    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len())
    }

    fn scroll_up(&mut self, n: usize) {
        if self.follow {
            self.follow = false;
            self.scroll = self.max_scroll;
        }
        self.scroll = self.scroll.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
        if self.scroll >= self.max_scroll {
            self.follow = true;
        }
    }

    fn push_line(&mut self, line: Line<'static>) {
        self.lines.push(line);
        self.follow = true;
    }

    fn push_user(&mut self, text: String) {
        let style = Style::default().fg(Color::Cyan);
        for (i, l) in text.split('\n').enumerate() {
            let prefix = if i == 0 { "❯ " } else { "  " };
            self.push_line(Line::from(vec![
                Span::styled(prefix.to_string(), style.add_modifier(Modifier::BOLD)),
                Span::styled(l.to_string(), style),
            ]));
        }
    }

    fn push_assistant(&mut self, text: String) {
        for line in markdown_lines(&text) {
            self.push_line(line);
        }
    }

    fn push_thinking(&mut self, text: String) {
        let style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC);
        for l in text.split('\n') {
            self.push_line(Line::styled(l.to_string(), style));
        }
    }

    fn flush_thinking(&mut self) {
        if !self.thinking.is_empty() {
            let text = std::mem::take(&mut self.thinking);
            self.push_thinking(text);
        }
    }

    /// Replay a loaded session's transcript so a resume is visibly restored.
    fn replay_history(&mut self) {
        let messages = self.session.messages();
        for message in &messages {
            match message {
                Message::User { content, .. } => {
                    let text = content
                        .iter()
                        .filter_map(|c| c.as_text())
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.is_empty() {
                        self.push_user(text);
                    }
                }
                Message::Assistant(a) => {
                    for c in &a.content {
                        match c {
                            Content::Thinking { thinking, .. } => {
                                if !thinking.is_empty() {
                                    self.push_thinking(thinking.clone());
                                }
                            }
                            Content::Text { text } => {
                                if !text.is_empty() {
                                    self.push_assistant(text.clone());
                                }
                            }
                            Content::ToolCall {
                                name, arguments, ..
                            } => {
                                self.push_tool(format!(
                                    "→ {} {}",
                                    name,
                                    slash::truncate(&arguments.to_string(), 160)
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                Message::ToolResult(tr) => {
                    let status = if tr.is_error { "error" } else { "ok" };
                    self.push_tool(format!("← {} {status}", tr.tool_name));
                }
            }
        }
    }

    fn push_system(&mut self, text: String) {
        let style = Style::default().fg(Color::DarkGray);
        for l in text.split('\n') {
            self.push_line(Line::styled(l.to_string(), style));
        }
    }

    fn push_error(&mut self, text: String) {
        let style = Style::default().fg(Color::Red);
        for l in text.split('\n') {
            self.push_line(Line::styled(l.to_string(), style));
        }
    }

    fn push_tool(&mut self, text: String) {
        let style = Style::default().fg(Color::Yellow);
        self.push_line(Line::styled(text, style));
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let input_rows = self
            .input_layout(area.width.saturating_sub(2))
            .0
            .clamp(1, MAX_INPUT_ROWS);
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(input_rows as u16 + 2),
            Constraint::Length(2),
        ])
        .split(area);

        self.render_title(frame, chunks[0]);
        self.render_chat(frame, chunks[1]);
        self.render_input(frame, chunks[2]);
        self.render_footer(frame, chunks[3]);

        if self.pending_perm.is_some() {
            self.render_permission(frame, area);
        }
        self.render_picker(frame, area);
        self.render_tree(frame, area);
    }

    fn render_title(&self, frame: &mut ratatui::Frame, area: Rect) {
        let id: String = self.session.id.chars().take(8).collect();
        let line = Line::from(vec![
            Span::styled(
                " pi ",
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{} ({})", self.app.model.name, self.app.model.provider),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   session {id}"),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_chat(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let mut lines = self.lines.clone();
        if !self.thinking.is_empty() {
            let style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC);
            for l in self.thinking.split('\n') {
                lines.push(Line::styled(l.to_string(), style));
            }
        }
        if !self.streaming.is_empty() {
            for l in self.streaming.split('\n') {
                lines.push(Line::from(l.to_string()));
            }
        }
        let paragraph = Paragraph::new(Text::from(lines))
            .block(Block::bordered().title(" conversation "))
            .wrap(Wrap { trim: false });

        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2) as usize;
        let total = paragraph.line_count(inner_width);
        self.max_scroll = total.saturating_sub(inner_height);
        let scroll = if self.follow {
            self.max_scroll
        } else {
            self.scroll.min(self.max_scroll)
        };
        let paragraph = paragraph.scroll((scroll.min(u16::MAX as usize) as u16, 0));
        frame.render_widget(paragraph, area);
    }

    fn render_input(&self, frame: &mut ratatui::Frame, area: Rect) {
        let title = if self.busy {
            " message (busy — Ctrl+C to interrupt) "
        } else {
            " message · Enter send · Shift+Enter newline · /help "
        };
        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2) as usize;
        let (_, cursor_row, cursor_col) = self.input_layout(inner_width);
        let paragraph = Paragraph::new(self.input.clone())
            .block(Block::bordered().title(title))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
        if self.pending_perm.is_none()
            && self.picker.is_none()
            && self.tree.is_none()
            && cursor_row < inner_height
        {
            let x = area
                .x
                .saturating_add(1)
                .saturating_add(cursor_col as u16)
                .min(area.x + area.width.saturating_sub(2));
            frame.set_cursor_position(Position::new(x, area.y + 1 + cursor_row as u16));
        }
    }

    /// Wrapped layout of the input: `(rows, cursor_row, cursor_col)` in display
    /// columns, so wide (CJK) characters and multi-line input keep the cursor
    /// and the terminal's IME anchor aligned. Explicit `\n` starts a row;
    /// exceeding `inner_width` wraps to the next one.
    fn input_layout(&self, inner_width: u16) -> (usize, usize, usize) {
        let width = inner_width.max(1) as usize;
        let total = self.input.chars().count();
        let mut rows = 1usize;
        let mut row = 0usize;
        let mut col = 0usize;
        let mut cursor_row = 0usize;
        let mut cursor_col = 0usize;
        for (i, ch) in self.input.chars().enumerate() {
            if i == self.cursor {
                cursor_row = row;
                cursor_col = col;
            }
            if ch == '\n' {
                row += 1;
                col = 0;
            } else {
                let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                if col + cw > width {
                    row += 1;
                    col = 0;
                }
                col += cw;
            }
            rows = rows.max(row + 1);
        }
        if self.cursor >= total {
            cursor_row = row;
            cursor_col = col;
        }
        (rows, cursor_row, cursor_col)
    }

    /// Bottom status bar, mirroring upstream `pi`: a dim `pwd (git-branch)`
    /// line, then usage stats on the left and `model • thinking` on the right.
    fn render_footer(&self, frame: &mut ratatui::Frame, area: Rect) {
        let dim = Style::default().fg(Color::DarkGray);

        let mut first = String::new();
        if self.busy {
            first.push_str(SPINNER[self.spinner % SPINNER.len()]);
            first.push(' ');
        }
        first.push_str(&self.pwd_display());
        let line1 = Line::styled(first, dim);

        let uses_subscription = self.app.model.provider == "kimi-coding";
        let right = if self.app.model.reasoning {
            let level = thinking_label(self.app.thinking_level);
            let suffix = if level == "off" {
                "thinking off".to_string()
            } else {
                level.to_string()
            };
            format!("{} • {suffix}", self.app.model.id)
        } else {
            self.app.model.id.clone()
        };

        let left = if self.pending_perm.is_some() {
            Span::styled(
                "permission required — y / a / n".to_string(),
                Style::default().fg(Color::Yellow),
            )
        } else {
            Span::styled(self.stats_line(uses_subscription), dim)
        };

        let line2 = padded_line(vec![left], Span::styled(right, dim), area.width);
        frame.render_widget(Paragraph::new(Text::from(vec![line1, line2])), area);
    }

    fn pwd_display(&self) -> String {
        let cwd = std::env::current_dir().unwrap_or_default();
        let mut path = cwd.to_string_lossy().to_string();
        if let Some(home) = dirs::home_dir() {
            let home = home.to_string_lossy().to_string();
            let sep = std::path::MAIN_SEPARATOR;
            if path == home {
                path = "~".to_string();
            } else if let Some(rest) = path.strip_prefix(&format!("{home}{sep}")) {
                path = format!("~{sep}{rest}");
            }
        }
        match &self.git_branch {
            Some(branch) => format!("{path} ({branch})"),
            None => path,
        }
    }

    /// Cumulative token usage across every assistant entry in the session.
    fn usage_totals(&self) -> (u64, u64, u64, u64, f64) {
        let mut input = 0;
        let mut output = 0;
        let mut cache_read = 0;
        let mut cache_write = 0;
        let mut cost = 0.0;
        for entry in &self.session.entries {
            if let Some(Message::Assistant(a)) = &entry.message {
                input += a.usage.input;
                output += a.usage.output;
                cache_read += a.usage.cache_read;
                cache_write += a.usage.cache_write;
                cost += a.usage.cost.total;
            }
        }
        (input, output, cache_read, cache_write, cost)
    }

    /// Tokens the latest request occupied, as a percentage of the context window.
    fn context_percent(&self) -> Option<f64> {
        let last = self
            .session
            .entries
            .iter()
            .rev()
            .find_map(|e| match &e.message {
                Some(Message::Assistant(a)) => Some(&a.usage),
                _ => None,
            })?;
        let used = last.input + last.cache_read + last.cache_write + last.output;
        let window = self.app.model.context_window as f64;
        if used > 0 && window > 0.0 {
            Some(used as f64 / window * 100.0)
        } else {
            None
        }
    }

    fn stats_line(&self, uses_subscription: bool) -> String {
        let (input, output, cache_read, cache_write, cost) = self.usage_totals();
        let mut parts: Vec<String> = Vec::new();
        if input > 0 {
            parts.push(format!("↑{}", format_tokens(input)));
        }
        if output > 0 {
            parts.push(format!("↓{}", format_tokens(output)));
        }
        if cache_read > 0 {
            parts.push(format!("R{}", format_tokens(cache_read)));
        }
        if cache_write > 0 {
            parts.push(format!("W{}", format_tokens(cache_write)));
        }
        if cost > 0.0 || uses_subscription {
            parts.push(format!(
                "${cost:.3}{}",
                if uses_subscription { " (sub)" } else { "" }
            ));
        }
        let window = format_tokens(self.app.model.context_window as u64);
        parts.push(match self.context_percent() {
            Some(p) => format!("{p:.1}%/{window}"),
            None => format!("?/{window}"),
        });
        parts.join(" ")
    }

    fn render_permission(&self, frame: &mut ratatui::Frame, area: Rect) {
        let Some(pending) = &self.pending_perm else {
            return;
        };
        let popup = centered_rect(72, 60, area);
        frame.render_widget(Clear, popup);
        let text = format!(
            "tool: {}\n\n{}\n\n[y] allow   [a] allow for session   [n] deny",
            pending.tool_name,
            slash::truncate(&pending.args, 1200)
        );
        let paragraph = Paragraph::new(text)
            .block(
                Block::bordered()
                    .title(" permission required ")
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, popup);
    }

    fn render_picker(&self, frame: &mut ratatui::Frame, area: Rect) {
        let Some(picker) = &self.picker else {
            return;
        };
        let popup = centered_rect(80, 70, area);
        frame.render_widget(Clear, popup);
        let inner_height = popup.height.saturating_sub(2) as usize;
        let start = picker
            .selected
            .saturating_sub(inner_height.saturating_sub(1));
        let lines: Vec<Line> = picker
            .sessions
            .iter()
            .enumerate()
            .skip(start)
            .take(inner_height)
            .map(|(i, s)| {
                let id: String = s.id.chars().take(8).collect();
                let text = format!(
                    "{id}  {:>3} msgs  {:<16}  {}  {}",
                    s.turns,
                    slash::truncate(&s.model, 16),
                    short_time(s.updated_ms),
                    slash::truncate(&s.first_message, 34)
                );
                if i == picker.selected {
                    Line::styled(
                        text,
                        Style::default()
                            .bg(Color::Cyan)
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    Line::from(text)
                }
            })
            .collect();
        let block = Block::bordered()
            .title(" resume a session   ↑/↓ · Enter · Esc new ")
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block), popup);
    }

    fn render_tree(&self, frame: &mut ratatui::Frame, area: Rect) {
        let Some(tree) = &self.tree else {
            return;
        };
        let popup = centered_rect(86, 76, area);
        frame.render_widget(Clear, popup);
        let rows = build_tree_rows(tree, self.session.active_leaf.as_deref());
        let inner_height = popup.height.saturating_sub(2) as usize;
        let list_height = inner_height.saturating_sub(1);
        let position = rows
            .iter()
            .position(|r| r.item == tree.selected)
            .unwrap_or(0);
        let start = position.saturating_sub(list_height.saturating_sub(1));
        let end = (start + list_height).min(rows.len());

        let mut lines: Vec<Line> = Vec::new();
        for row in &rows[start..end] {
            let item = &tree.items[row.item];
            let selected = row.item == tree.selected;
            let connector_shown = row.show_connector && !row.is_virtual_root_child;
            let connector_position = row.display_indent.saturating_sub(1);
            let mut prefix = String::new();
            for i in 0..row.display_indent * 3 {
                let level = i / 3;
                let pos = i % 3;
                if let Some((_, show)) = row.gutters.iter().find(|(p, _)| *p == level) {
                    prefix.push(if pos == 0 {
                        if *show {
                            '│'
                        } else {
                            ' '
                        }
                    } else {
                        ' '
                    });
                } else if connector_shown && level == connector_position {
                    match pos {
                        0 => prefix.push(if row.is_last { '└' } else { '├' }),
                        1 => prefix.push(if row.folded {
                            '⊞'
                        } else if row.foldable {
                            '⊟'
                        } else {
                            '─'
                        }),
                        _ => prefix.push(' '),
                    }
                } else {
                    prefix.push(' ');
                }
            }
            let fold_marker = if row.folded && !connector_shown {
                "⊞ "
            } else {
                ""
            };
            let path_marker = if row.on_active_path { "• " } else { "" };
            let cursor = if selected { "› " } else { "  " };

            if selected {
                let text = format!("{cursor}{prefix}{fold_marker}{path_marker}{}", item.label);
                lines.push(Line::styled(
                    text,
                    Style::default()
                        .bg(Color::Cyan)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                lines.push(Line::from(vec![
                    Span::raw(cursor),
                    Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{fold_marker}{path_marker}"),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(item.label.clone(), Style::default()),
                ]));
            }
        }

        lines.push(Line::styled(
            format!(
                "  ({}/{}) {}",
                if rows.is_empty() { 0 } else { position + 1 },
                rows.len(),
                tree.filter.label()
            ),
            Style::default().fg(Color::DarkGray),
        ));

        let title = match tree.mode {
            TreeMode::Switch => {
                " session tree   ↑/↓ · ←/→ page · ctrl+←/→ fold · ctrl+o filter · Enter select · Esc "
            }
            TreeMode::Fork => " fork from a user message   ↑/↓ · Enter fork · Esc ",
        };
        let block = Block::bordered()
            .title(title)
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block), popup);
    }
}

/// A line with `left` spans flush-left and `right` flush-right, padded by
/// display width so wide (CJK) text and the spinner stay aligned.
fn padded_line(mut left: Vec<Span<'static>>, right: Span<'static>, width: u16) -> Line<'static> {
    let left_w: usize = left
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let right_w = UnicodeWidthStr::width(right.content.as_ref());
    let gap = (width as usize).saturating_sub(left_w + right_w).max(1);
    left.push(Span::raw(" ".repeat(gap)));
    left.push(right);
    Line::from(left)
}

/// Compact token counts, mirroring upstream `pi`.
fn format_tokens(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=9_999 => format!("{:.1}k", count as f64 / 1000.0),
        10_000..=999_999 => format!("{}k", (count as f64 / 1000.0).round() as u64),
        1_000_000..=9_999_999 => format!("{:.1}M", count as f64 / 1_000_000.0),
        _ => format!("{}M", (count as f64 / 1_000_000.0).round() as u64),
    }
}

fn thinking_label(level: pi_ai::ThinkingLevel) -> &'static str {
    match level {
        pi_ai::ThinkingLevel::Off => "off",
        pi_ai::ThinkingLevel::Minimal => "minimal",
        pi_ai::ThinkingLevel::Low => "low",
        pi_ai::ThinkingLevel::Medium => "medium",
        pi_ai::ThinkingLevel::High => "high",
        pi_ai::ThinkingLevel::Xhigh => "xhigh",
    }
}

/// Current git branch (cached for the footer), if in a repo with a branch.
fn detect_git_branch() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        None
    } else {
        Some(branch)
    }
}

/// `MM-DD HH:MM` in local time for the picker.
fn short_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(area);
    let [_, middle, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(middle);
    middle
}

/// Very small Markdown renderer: fenced code blocks, `#` headings, `**bold**`
/// and inline `code`. Keeps the transcript readable without pulling in a full
/// Markdown parser dependency.
fn markdown_lines(text: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in text.split('\n') {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(Line::styled(
                raw.to_string(),
                Style::default().fg(Color::DarkGray),
            ));
            continue;
        }
        if in_code {
            out.push(Line::styled(
                raw.to_string(),
                Style::default().fg(Color::Green),
            ));
            continue;
        }
        if raw.trim_start().starts_with('#') {
            out.push(Line::styled(
                raw.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        out.push(inline_markdown(raw));
    }
    out
}

fn inline_markdown(s: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let bold = find_pair(rest, "**");
        let code = find_pair(rest, "`");
        let pick = match (bold, code) {
            (Some(b), Some(c)) => Some(if b.0 <= c.0 { b } else { c }),
            (Some(b), None) => Some(b),
            (None, Some(c)) => Some(c),
            (None, None) => None,
        };
        let Some((start, inner_start, inner_len, marker_len)) = pick else {
            spans.push(Span::raw(rest.to_string()));
            break;
        };
        if start > 0 {
            spans.push(Span::raw(rest[..start].to_string()));
        }
        let style = if marker_len == 2 {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        spans.push(Span::styled(
            rest[inner_start..inner_start + inner_len].to_string(),
            style,
        ));
        rest = &rest[inner_start + inner_len + marker_len..];
    }
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    Line::from(spans)
}

/// Find `open` ... `close`; returns `(start, inner_start, inner_len, marker_len)`.
fn find_pair(s: &str, marker: &str) -> Option<(usize, usize, usize, usize)> {
    let start = s.find(marker)?;
    let inner_start = start + marker.len();
    let inner_len = s[inner_start..].find(marker)?;
    Some((start, inner_start, inner_len, marker.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> State {
        let app = AppConfig::default();
        let (permission, _rx) = TuiPermission::new();
        State {
            session: Session::new(&app.model),
            app,
            permission,
            system_prompt: String::new(),
            lines: Vec::new(),
            streaming: String::new(),
            thinking: String::new(),
            input: String::new(),
            cursor: 0,
            follow: true,
            scroll: 0,
            max_scroll: 0,
            busy: false,
            spinner: 0,
            quit: false,
            pending_perm: None,
            picker: None,
            tree: None,
            git_branch: None,
            branch_len: 0,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn edits_input_buffer() {
        let mut state = test_state();
        let (tx, _rx) = mpsc::unbounded_channel();
        state.on_key(key(KeyCode::Char('h')), &tx);
        state.on_key(key(KeyCode::Char('i')), &tx);
        assert_eq!(state.input, "hi");
        assert_eq!(state.cursor, 2);

        state.on_key(key(KeyCode::Left), &tx);
        state.on_key(key(KeyCode::Char('!')), &tx);
        assert_eq!(state.input, "h!i");
        assert_eq!(state.cursor, 2);

        state.on_key(key(KeyCode::Backspace), &tx);
        assert_eq!(state.input, "hi");
        state.on_key(key(KeyCode::Home), &tx);
        state.on_key(key(KeyCode::Delete), &tx);
        assert_eq!(state.input, "i");
    }

    #[test]
    fn accumulates_streaming_text() {
        let mut state = test_state();
        state.on_agent_event(AgentEvent::TextDelta {
            delta: "hello ".into(),
        });
        state.on_agent_event(AgentEvent::TextDelta {
            delta: "world".into(),
        });
        assert_eq!(state.streaming, "hello world");
    }

    #[test]
    fn tool_events_append_transcript_lines() {
        let mut state = test_state();
        state.on_agent_event(AgentEvent::ToolExecutionStart {
            tool_call_id: "1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "a.txt"}),
        });
        state.on_agent_event(AgentEvent::ToolExecutionEnd {
            tool_call_id: "1".into(),
            tool_name: "read".into(),
            is_error: false,
            content: Vec::new(),
        });
        assert_eq!(state.lines.len(), 2);
        assert!(line_text(&state.lines[0]).contains("read"));
        assert!(line_text(&state.lines[1]).contains("ok"));
    }

    #[test]
    fn answers_permission_shortcut() {
        let mut state = test_state();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (respond, mut answer) = oneshot::channel();
        state.pending_perm = Some(PendingPermission {
            tool_name: "bash".into(),
            args: "{}".into(),
            respond,
        });
        state.on_key(key(KeyCode::Char('a')), &tx);
        assert!(state.pending_perm.is_none());
        assert!(matches!(
            answer.try_recv(),
            Ok(PermissionDecision::AllowSession)
        ));
    }

    #[test]
    fn scroll_follows_bottom_until_user_scrolls() {
        let mut state = test_state();
        state.max_scroll = 5;
        state.scroll_up(1);
        assert!(!state.follow);
        assert_eq!(state.scroll, 4);
        state.scroll_down(10);
        assert!(state.follow);
    }

    #[test]
    fn cursor_column_counts_cjk_as_two_columns() {
        let mut state = test_state();
        let (tx, _rx) = mpsc::unbounded_channel();
        for c in "中文".chars() {
            state.on_key(key(KeyCode::Char(c)), &tx);
        }
        let (rows, cursor_row, cursor_col) = state.input_layout(20);
        assert_eq!(rows, 1);
        assert_eq!(cursor_row, 0);
        // Two wide glyphs → the cursor sits at display column 4, not 2.
        assert_eq!(cursor_col, 4);
    }

    #[test]
    fn wide_text_wraps_to_next_row() {
        let mut state = test_state();
        for c in "一二三四五六".chars() {
            state.input.push(c);
            state.cursor += 1;
        }
        // 12 display columns in a 6-column box → wraps to two rows.
        let (rows, cursor_row, cursor_col) = state.input_layout(6);
        assert_eq!(rows, 2);
        assert_eq!(cursor_row, 1);
        assert_eq!(cursor_col, 6);
    }

    #[test]
    fn shift_enter_inserts_newline() {
        let mut state = test_state();
        let (tx, _rx) = mpsc::unbounded_channel();
        state.on_key(key(KeyCode::Char('a')), &tx);
        state.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), &tx);
        state.on_key(key(KeyCode::Char('b')), &tx);
        assert_eq!(state.input, "a\nb");
        let (rows, cursor_row, _) = state.input_layout(20);
        assert_eq!(rows, 2);
        assert_eq!(cursor_row, 1);
    }

    #[test]
    fn markdown_styles_bold_and_code() {
        let line = inline_markdown("a **b** `c`");
        assert_eq!(line.spans.len(), 4);
        assert_eq!(line.spans[1].content.as_ref(), "b");
        assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[3].content.as_ref(), "c");
        assert_eq!(line.spans[3].style.fg, Some(Color::Yellow));
    }

    #[test]
    fn interrupt_records_partial_assistant() {
        let mut state = test_state();
        state.streaming = "partial answer".into();
        state.record_partial_assistant();
        assert!(state.streaming.is_empty());
        let messages = state.session.messages();
        match messages.last() {
            Some(Message::Assistant(a)) => {
                assert_eq!(a.stop_reason, pi_ai::StopReason::Aborted);
                assert_eq!(a.content[0].as_text(), Some("partial answer"));
            }
            other => panic!("expected assistant, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_c_persists_session() {
        let dir = std::env::temp_dir().join(format!(
            "pi-rs-tui-save-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut state = test_state();
        state.app.config_dir = dir.clone();
        state.session.push_message(Message::user_text("hello"));
        state.streaming = "partial".into();
        state.busy = true;

        let (tx, _rx) = mpsc::unbounded_channel();
        state.on_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &tx,
        );
        assert!(state.quit);

        let path = dir
            .join("sessions")
            .join(format!("{}.json", state.session.id));
        let text = std::fs::read_to_string(&path).expect("session file written");
        assert!(text.contains("hello"));
        assert!(text.contains("partial"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn replay_history_renders_transcript() {
        let mut state = test_state();
        state.session.push_message(Message::user_text("hello"));
        state
            .session
            .push_message(Message::Assistant(pi_ai::AssistantMessage {
                content: vec![Content::text("world")],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-flash".into(),
                usage: pi_ai::Usage::default(),
                stop_reason: pi_ai::StopReason::Stop,
                error_message: None,
                timestamp: pi_ai::now_ms(),
            }));
        state.replay_history();
        assert_eq!(state.lines.len(), 2);
        assert!(line_text(&state.lines[0]).contains("hello"));
        assert!(line_text(&state.lines[1]).contains("world"));
    }

    fn summary(id: &str) -> crate::session::SessionSummary {
        crate::session::SessionSummary {
            id: id.into(),
            updated_ms: 0,
            model: "deepseek-v4-flash".into(),
            provider: "deepseek".into(),
            first_message: "hi".into(),
            turns: 1,
        }
    }

    #[test]
    fn picker_wraps_selection() {
        let mut state = test_state();
        state.picker = Some(Picker {
            sessions: vec![summary("a"), summary("b"), summary("c")],
            selected: 0,
        });
        state.picker_move(-1);
        assert_eq!(state.picker.as_ref().unwrap().selected, 2);
        state.picker_move(1);
        assert_eq!(state.picker.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn picker_confirm_loads_selected_session() {
        let dir = std::env::temp_dir().join(format!(
            "pi-rs-picker-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let model = pi_ai::Model::openai_gpt_4o();
        let mut saved = Session::new(&model);
        saved.push_message(Message::user_text("from picker"));
        crate::session::save(&dir, &mut saved).unwrap();

        let mut state = test_state();
        state.app.config_dir = dir.clone();
        state.picker = Some(Picker {
            sessions: vec![summary(&saved.id)],
            selected: 0,
        });
        state.picker_confirm();
        assert!(state.picker.is_none());
        assert_eq!(state.session.id, saved.id);
        assert_eq!(state.session.messages().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    fn assistant(text: &str) -> Message {
        Message::Assistant(pi_ai::AssistantMessage {
            content: vec![Content::text(text)],
            api: "openai-completions".into(),
            provider: "deepseek".into(),
            model: "deepseek-v4-flash".into(),
            usage: pi_ai::Usage::default(),
            stop_reason: pi_ai::StopReason::Stop,
            error_message: None,
            timestamp: pi_ai::now_ms(),
        })
    }

    #[test]
    fn tree_switch_to_same_leaf_keeps_branch() {
        let mut state = test_state();
        state.session.push_message(Message::user_text("a"));
        state.session.push_message(assistant("b"));
        let (tx, _rx) = mpsc::unbounded_channel();
        state.open_tree(TreeMode::Switch);
        // Starts on the active leaf.
        assert_eq!(state.tree.as_ref().unwrap().selected, 1);
        state.tree_confirm(&tx);
        assert!(state.tree.is_none());
        assert_eq!(state.session.messages().len(), 2);
    }

    #[test]
    fn tree_fork_starts_new_session_with_prefix() {
        let mut state = test_state();
        let first = state.session.push_message(Message::user_text("first"));
        state.session.push_message(assistant("ok"));
        let second = state.session.push_message(Message::user_text("second"));
        let original_id = state.session.id.clone();

        state.open_tree(TreeMode::Fork);
        // Only user entries are visible (UserOnly filter).
        let tree = state.tree.as_ref().unwrap();
        let rows = build_tree_rows(tree, state.session.active_leaf.as_deref());
        assert_eq!(rows.len(), 2);
        // Starts on the active leaf ("second").
        assert_eq!(tree.items[tree.selected].id, second);
        assert_ne!(tree.items[tree.selected].id, first);

        let (tx, _rx) = mpsc::unbounded_channel();
        state.tree_confirm(&tx);

        assert_ne!(state.session.id, original_id);
        // Prefix is the branch up to the selected message's parent (first + ok).
        assert_eq!(state.session.messages().len(), 2);
        assert_eq!(state.input, "second");
    }

    #[test]
    fn tree_select_user_moves_to_parent_and_preloads_prompt() {
        let mut state = test_state();
        state.app.summarize_branches = false;
        state.session.push_message(Message::user_text("root"));
        let reply = state.session.push_message(assistant("reply"));
        let follow_up = state.session.push_message(Message::user_text("follow up"));
        state.session.push_message(assistant("more"));

        state.open_tree(TreeMode::Switch);
        let tree = state.tree.as_mut().unwrap();
        tree.selected = tree.items.iter().position(|it| it.id == follow_up).unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();
        state.tree_confirm(&tx);

        // The leaf moved to the selected user message's parent, and the prompt
        // is back in the editor for editing/resubmission.
        assert_eq!(state.session.active_leaf.as_deref(), Some(reply.as_str()));
        assert_eq!(state.input, "follow up");
    }

    #[test]
    fn tree_fold_hides_children() {
        let mut state = test_state();
        state.session.push_message(Message::user_text("a"));
        state.session.push_message(assistant("b"));
        state.session.push_message(Message::user_text("c"));
        state.open_tree(TreeMode::Switch);
        let rows = |s: &State| {
            build_tree_rows(s.tree.as_ref().unwrap(), s.session.active_leaf.as_deref()).len()
        };
        assert_eq!(rows(&state), 3);

        state.tree.as_mut().unwrap().selected = 0;
        state.tree_collapse();
        assert_eq!(rows(&state), 1);

        state.tree_expand();
        assert_eq!(rows(&state), 3);
    }

    #[test]
    fn tree_indents_only_at_branch_points() {
        // a(user) -> b(assistant) -> { c(user), d(user) }
        let mut state = test_state();
        state.session.push_message(Message::user_text("a"));
        let b = state.session.push_message(assistant("b"));
        state.session.push_message(Message::user_text("c"));
        state.session.set_active_leaf(Some(b));
        state.session.push_message(Message::user_text("d"));
        state.open_tree(TreeMode::Switch);

        let tree = state.tree.as_ref().unwrap();
        let rows = build_tree_rows(tree, state.session.active_leaf.as_deref());
        assert_eq!(rows.len(), 4);
        // a and b are a single-child chain: flat, no connector.
        assert_eq!(rows[0].display_indent, 0);
        assert!(!rows[0].show_connector);
        assert_eq!(rows[1].display_indent, 0);
        // c and d are the branch under b: indent 1, `├─` then `└─`.
        assert_eq!(rows[2].display_indent, 1);
        assert!(rows[2].show_connector);
        assert!(!rows[2].is_last);
        assert!(rows[3].is_last);
        assert_eq!(rows[3].display_indent, 1);
    }

    #[test]
    fn new_command_clears_transcript() {
        let mut state = test_state();
        state.lines.push(Line::from("old line"));
        state.session.push_message(Message::user_text("old"));
        let (tx, _rx) = mpsc::unbounded_channel();
        state.run_slash("/new".to_string(), &tx);
        assert!(state.session.is_empty());
        // Only the re-announced model line remains.
        assert_eq!(state.lines.len(), 1);
    }

    #[test]
    fn padded_line_aligns_right_edge() {
        let line = padded_line(vec![Span::raw("ready")], Span::raw("hint"), 20);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "ready           hint");
        assert_eq!(UnicodeWidthStr::width(text.as_str()), 20);
    }

    #[test]
    fn padded_line_counts_cjk_width() {
        // "工作" occupies 4 display columns, so the right edge still lands on 10.
        let line = padded_line(vec![Span::raw("工作")], Span::raw("x"), 10);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(UnicodeWidthStr::width(text.as_str()), 10);
    }

    #[test]
    fn format_tokens_is_compact() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1234), "1.2k");
        assert_eq!(format_tokens(12_345), "12k");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }
}
