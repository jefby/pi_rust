# TUI

A full-screen terminal UI is available behind the optional `tui` cargo
feature. It replaces the line REPL with a scrollable conversation pane, an
input box, inline tool status, and a permission modal.

```bash
cargo build --release -p pi-coding-agent --features tui
./target/release/pi
```

The feature is **off by default** so the default binary stays small. The
official [prebuilt Windows binary](../../../prebuilt/win_x64/) is built with
it enabled.

## Behavior

- When the `tui` feature is compiled in and both stdin and stdout are a TTY,
  `pi` starts the TUI automatically.
- `--no-tui` forces the line REPL.
- `--tui` only *requires* a TTY; on a pipe it warns and falls back to the REPL.
- `-p` / `--json` print mode is unaffected.

## Keys

| Key | Action |
|-----|--------|
| `Enter` | send the message |
| `Shift+Enter` | insert a newline (multi-line input) |
| `PageUp` / `PageDown` / `Up` / `Down` | scroll the conversation |
| `Ctrl+C` | save and quit (an in-flight turn is recorded as an aborted message) |
| `Ctrl+D` | quit when the input is empty |
| `y` / `a` / `n` | answer a permission prompt (allow / allow session / deny) |

## Status bar

The bottom status bar mirrors upstream `pi`:

```
~/github_code/pi_rust (feat/windows)
↑12.3k ↓4.5k R1.2k $0.031  42.3%/200k               deepseek-v4-flash • high
```

- Line 1: the working directory (`~` for home) plus the git branch, with a
  spinner while a turn is running.
- Line 2: cumulative token usage (`↑` input, `↓` output, `R` cache read,
  `W` cache write), USD cost, and context-window usage on the left;
  `model • thinking-level` on the right.
- A pending permission prompt temporarily replaces the left of line 2 with
  `permission required — y / a / n`.

Key hints (`Enter send`, `Shift+Enter newline`, `/help`) live in the input box
title.

## Session tree

`/tree` opens the session as a tree, rendered like upstream `pi`: only branch
points indent and draw connectors (`├─`/`└─`, `─`/`⊟`/`⊞`), single-child chains
stay flat, the active branch is ordered first, and the current path is marked
with `•`.

| Key | Action |
|-----|--------|
| `↑` / `↓` | move the selection |
| `←` / `→` | page up / down |
| `Ctrl+←` / `Ctrl+→` (or `Alt+←`/`Alt+→`) | fold / unfold |
| `Ctrl+O` | cycle filter: `default`, `no-tools`, `user-only`, `all` |
| `Enter` | select |
| `Esc` | cancel |

Selection matches upstream: choosing a **user** message moves the leaf to its
parent and puts that prompt back in the editor (edit & resubmit creates a new
branch); choosing any other entry moves the leaf to that entry. The abandoned
branch is summarized when branch summaries are enabled. The footer shows
`(selected/total) filter`.

`/fork` opens the same view restricted to user messages and starts a new
session from the selected one, with that prompt placed back in the input. See
[Sessions](./sessions.md#branching-session-tree).

The conversation pane renders model reasoning (thinking deltas) dimmed and
italic, applies light Markdown styling to assistant messages (fenced code
blocks, headings, `**bold**`, inline `code`), and shows tool runs inline.

## Resume picker

Running `pi -r` with no argument opens a picker listing recent sessions
(id, message count, model, time, first prompt):

| Key | Action |
|-----|--------|
| `Up` / `Down` (or `k` / `j`) | move the selection |
| `Enter` | resume the selected session |
| `Esc` | start a new session |

When the picker is dismissed or no sessions exist, `pi` starts a fresh
session. In the line REPL (no TTY) a bare `-r` resumes the most recent
session instead.

## Slash commands

The same commands as the [REPL](./interactive.md#slash-commands) are
supported, including `/compact`.

## Wide characters and IME

The input box measures text in terminal display columns, so CJK/emoji
characters (which are two columns wide) keep the cursor aligned and long
lines scroll horizontally instead of running off the edge.

Input-method (IME) composition itself is handled by the terminal. The app
positions the real terminal cursor, which is where most terminals anchor the
candidate window. If your terminal shows no candidate window in the
full-screen buffer, use `--no-tui` — the line REPL uses the terminal's native
line editor and IME integration.

## Size

Enabling the feature links [`ratatui`](https://crates.io/crates/ratatui) and
[`crossterm`](https://crates.io/crates/crossterm). On
`x86_64-pc-windows-gnu` the stripped release binary grows from ~10.8 MiB to
~11.7 MiB (roughly +0.9 MiB, +8%).
