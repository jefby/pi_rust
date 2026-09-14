# Options

```text
pi [OPTIONS] [COMMAND]
```

## Model & provider

| Flag | Description |
|------|-------------|
| `-m, --model <MODEL>` | Model id or alias. Also read from `PI_MODEL`. Overrides `config.toml` and the upstream default. |
| `--provider <NAME>` | Force a provider from the built-in table or the upstream `~/.pi/agent` config. |
| `--api-key <KEY>` | Override the API key for the selected provider. |
| `--list-models [SEARCH]` | List available `provider/model` ids (optional case-insensitive search) and exit. |
| `--thinking <LEVEL>` | Extended-thinking budget: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`. |
| `--max-turns <N>` | Maximum agent turns before stopping. |

## Session

| Flag | Description |
|------|-------------|
| `--resume <ID>` | Load a saved session by id. |
| `-c, --continue` | Continue the most recently updated session. |

## Prompt

| Flag | Description |
|------|-------------|
| `--system-prompt <TEXT>` | Replace the default system prompt. |
| `--append-system-prompt <TEXT>` | Append to the system prompt (repeatable). |
| `--no-context-files` | Disable `AGENTS.md` / `CLAUDE.md` discovery. |

## Modes

| Flag | Description |
|------|-------------|
| `-p, --prompt <PROMPT>` | One-shot print mode: run to completion and exit. |
| `--json` | In print mode, emit JSON-lines instead of human text. |
| `--yolo` | Skip permission prompts (dangerous). |
| `--tui` | Require the full-screen TUI (needs `--features tui` + a TTY). |
| `--no-tui` | Force the line REPL. |

## Subcommands

```text
pi sessions list
pi sessions show <id>
pi sessions delete <id>
```

## Model resolution

The active model is resolved in this order:

1. `-m` / `PI_MODEL` / `--provider` (+ `-m`)
2. `config.toml` (`model`)
3. custom providers in the upstream `models.json`
4. the upstream `settings.json` default provider + model
5. environment-key fallback

Run with `RUST_LOG=debug` to print the resolved provider, model, API, base URL,
and whether a key was found.
