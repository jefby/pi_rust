# Interactive

Running `pi` with no arguments drops you into the REPL. Assistant text
streams in as it arrives; tool calls show a confirmation prompt by default
(see [Permissions](./permissions.md)). After every turn the session is
persisted to disk (see [Sessions](./sessions.md)).

```bash
pi
```

> When `pi` is built with `--features tui` and run on an interactive terminal,
> it starts the full-screen [TUI](./tui.md) instead of the line REPL. Pass
> `--no-tui` to force the REPL.

## Slash commands

```text
/help                show command list
/quit  /exit         quit pi
/new                 start a new session
/reset               start a new session (alias of /new)
/model               print the active model
/tools               list builtin tools
/cost                show accumulated token usage
/sessions            list saved sessions
/resume <id>         load a saved session by id
/session             print current session id
/tree                switch to an earlier point in the session tree (TUI)
/fork                start a new session from a previous user message (TUI)
/clone               duplicate the active branch into a new session
/compact             summarize older messages into a recap
```

The REPL also loads `AGENTS.md`, `CLAUDE.md`, and `.pi/instructions.md`
from the current directory upward and concatenates them into the system
prompt.
