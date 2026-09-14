# Sessions

Every interactive turn is saved to disk under
`$XDG_CONFIG_HOME/pi/sessions/<id>.json` (typically
`~/.config/pi/sessions/` on Linux/macOS, `%APPDATA%\pi\sessions\` on Windows).
Sessions contain the full message transcript and can be reloaded later.

## Subcommands

```bash
pi sessions list           # list saved sessions (id, model, last updated)
pi sessions show <id>      # print a session's transcript
pi sessions delete <id>    # delete a saved session
```

## Resume

To continue a previous conversation, pass `-r` / `--resume` with an id or a
file path (or the equivalent `--session`):

```bash
pi -r 0193abcd-...                    # by id
pi -r ~/.pi/agent/sessions/...jsonl  # upstream file
pi -r                                # TUI: pick from a list; REPL: most recent
pi --continue                        # most recently updated session
pi --session 0193abcd-...            # explicit-by-path alias
```

Inside the REPL you can also use `/resume <id>` to swap to a saved
session, or `/session` to print the current session id.

## When sessions are saved

The transcript is written to disk **before each turn runs** and again when the
turn finishes, so an interrupt (Ctrl+C) or crash still leaves a resumable
session. In the TUI, Ctrl+C records whatever the assistant streamed so far as
an aborted message before exiting.

## Interop with upstream `pi`

The upstream TypeScript `pi` stores sessions as **JSONL** under
`~/.pi/agent/sessions/--<cwd>--/<timestamp>_<uuid>.jsonl`, with message
entries linked into a tree via `id` / `parentId`. The Rust CLI can read and
write that format:

```bash
# Import an upstream .jsonl session into the local store.
pi sessions import ~/.pi/agent/sessions/--my-project--/2026-05-01T...jsonl

# Export a local session as upstream v3 JSONL (a new file; nothing under
# ~/.pi/agent is modified).
pi sessions export <id> --to ./my-session.jsonl
```

You can also load an upstream file directly without importing it:

```bash
pi --resume ~/.pi/agent/sessions/--my-project--/2026-05-01T...jsonl
```

Import walks the active branch (the last entry back to the root through
`parentId`) and flattens it into the local transcript, preserving text,
thinking, tool calls, tool results, usage, and stop reasons. Non-message
entries (`model_change`, `thinking_level_change`, `custom`) are skipped.

To make an exported file visible to the TypeScript `pi`, place it under
`~/.pi/agent/sessions/--<cwd-slug>--/`, where `<cwd-slug>` is the project
path with path separators replaced by `-`.
