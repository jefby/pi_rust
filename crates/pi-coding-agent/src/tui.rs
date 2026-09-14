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
use unicode_width::UnicodeWidthChar;

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
}

struct PendingPermission {
    tool_name: String,
    args: String,
    respond: oneshot::Sender<PermissionDecision>,
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
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, app, permission, permission_rx, initial).await;
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut DefaultTerminal,
    app: &AppConfig,
    permission: Arc<TuiPermission>,
    mut permission_rx: mpsc::UnboundedReceiver<PermissionRequest>,
    initial: Option<Session>,
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
    };
    if !state.session.messages.is_empty() {
        state.push_system(format!(
            "(resumed session {}, {} prior messages)",
            state.session.id,
            state.session.messages.len()
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
        // Ctrl+C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
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

        let cfg = AgentConfig::new(self.app.model.clone(), self.system_prompt.clone())
            .with_tools(default_tools())
            .with_max_turns(self.app.max_turns)
            .with_thinking(self.app.thinking_level)
            .with_api_key(self.app.api_key.clone())
            .with_permission(self.permission.clone());
        let user = Message::user_text(prompt);
        let mut history = self.session.messages.clone();
        history.push(user);

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
                if !outcome.keep_going {
                    self.quit = true;
                }
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
                self.session.replace_messages(run.messages);
                self.save();
            }
            Err(e) => self.push_error(format!("agent error: {e}")),
        }
        if !self.streaming.is_empty() {
            let text = std::mem::take(&mut self.streaming);
            self.push_assistant(text);
        }
        self.flush_thinking();
        self.busy = false;
    }

    fn save(&mut self) {
        if let Err(e) = crate::session::save(&self.app.config_dir, &self.session) {
            self.push_system(format!("(warning: session save failed: {e})"));
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
            Constraint::Length(1),
        ])
        .split(area);

        self.render_title(frame, chunks[0]);
        self.render_chat(frame, chunks[1]);
        self.render_input(frame, chunks[2]);
        self.render_status(frame, chunks[3]);

        if self.pending_perm.is_some() {
            self.render_permission(frame, area);
        }
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
            " message (busy) "
        } else {
            " message (Shift+Enter newline) "
        };
        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2) as usize;
        let (_, cursor_row, cursor_col) = self.input_layout(inner_width);
        let paragraph = Paragraph::new(self.input.clone())
            .block(Block::bordered().title(title))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
        if self.pending_perm.is_none() && cursor_row < inner_height {
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

    fn render_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        let left = if self.pending_perm.is_some() {
            "permission required — y / a / n".to_string()
        } else if self.busy {
            format!("{} working…", SPINNER[self.spinner % SPINNER.len()])
        } else {
            "ready".to_string()
        };
        let line = Line::from(vec![
            Span::styled(left, Style::default().fg(Color::Green)),
            Span::styled(
                "    Enter send • PgUp/PgDn scroll • Ctrl+C quit • /help",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
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
}
