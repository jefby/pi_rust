# Parity with upstream `pi`

How the Rust port (`pi-coding-agent` 1.2.0) compares with the TypeScript
upstream it is ported from.

> **Snapshot:** upstream `@earendil-works/pi-coding-agent` **0.85.1** vs this
> workspace at **1.2.0**. Recorded 2026-09-14. Upstream moves quickly; treat
> this as a point-in-time gap analysis, not a contract.

**Bottom line:** the Rust port is a focused subset — the core coding-agent
loop, the basic CLI, and a handful of providers. It is **not** a feature
equivalent of 0.85.1. Roughly 5.5k lines of Rust `src` against ~189k lines of
TypeScript.

Legend: ✅ parity · 🟡 partial · ❌ missing · ➕ Rust-only extra

## Built-in tools

| Tool | Upstream 0.85.1 | Rust 1.2.0 |
|------|:---------------:|:----------:|
| `read` | ✅ | ✅ |
| `write` | ✅ | ✅ |
| `edit` | ✅ | ✅ (unified-diff preview) |
| `bash` | ✅ | ✅ (streamed, persisted cwd, shell auto-detect) |
| `ls` | ✅ | ✅ |
| `grep` | ✅ | ✅ (regex + context lines) |
| `find` (glob) | ✅ | ✅ (tool is named `find`) |
| `powershell` | ✅ | 🟡 Windows only |
| `web_fetch` | ❌ (extension) | ➕ |
| `todo` | ❌ (extension) | ➕ |

## Providers

Upstream ships a catalog of **39 providers**:

`amazon-bedrock`, `ant-ling`, `anthropic`, `azure-openai-responses`, `baseten`,
`cerebras`, `cloudflare-ai-gateway`, `cloudflare-workers-ai`, `deepseek`,
`fireworks`, `github-copilot`, `google-vertex`, `google`, `groq`, `huggingface`,
`kimi-coding`, `minimax`, `minimax-cn`, `mistral`, `moonshotai`,
`moonshotai-cn`, `nvidia`, `openai-codex`, `openai`, `opencode`,
`opencode-go`, `openrouter`, `qwen-token-plan`, `qwen-token-plan-cn`,
`qwen-token-plan-individual`, `together`, `vercel-ai-gateway`, `xai`,
`xiaomi`, `xiaomi-token-plan-ams`, `xiaomi-token-plan-cn`,
`xiaomi-token-plan-sgp`, `zai`, `zai-coding-cn`.

| | Upstream | Rust |
|---|----------|------|
| Wire APIs implemented | `anthropic-messages`, `openai-completions`, `openai-responses`, `google-generative-ai`, `amazon-bedrock`, `google-vertex`, `azure-openai-responses`, custom APIs | `anthropic-messages`, `openai-completions`, `openai-responses`, `google-generative-ai` |
| Named providers | 39 | 4 native code paths + ~14 via the `~/.pi/agent` config bridge |
| OAuth / subscription login (`/login`) | ✅ | ❌ |
| Provider-specific quirks (cache markers, adaptive thinking, etc.) | ✅ | 🟡 partial (Anthropic cache markers only) |

The [upstream-config bridge](./install.md#reusing-the-upstream-pi-configuration)
maps a provider name to `api` + base URL and reuses the configured key, so an
OpenAI-compatible or Anthropic-compatible provider (DeepSeek, OpenRouter, Groq,
Kimi, ...) works. It does **not** add provider-specific behavior.

## CLI surface

| | Upstream 0.85.1 | Rust 1.2.0 |
|---|-----------------|------------|
| Subcommands | `install`, `remove`, `uninstall`, `update`, `list`, `config`, `auth` | `sessions list/show/delete` |
| Output modes | `text`, `json`, `rpc` | text, `--json` (print mode) |
| Options | ~40 | ~14 (see [Options](./cli/options.md)) |
| Provider/model selection | `--provider`, `--model <pattern>` (glob/`provider/id`), `--models`, `--list-models` | `-m` / `PI_MODEL` (alias or upstream default) |
| Tools control | `--tools`, `--exclude-tools`, `--no-tools`, `--no-builtin-tools` | ❌ |
| System prompt | `--system-prompt`, `--append-system-prompt` | ✅ |
| Sessions | `--continue`, `--resume`, `--session`, `--session-id`, `--fork`, `--session-dir`, `--no-session`, `--name` | 🟡 `--resume`, `--session`, `--continue`; upstream `.jsonl` import/export |
| Resources | `--extension`, `--skill`, `--prompt-template`, `--theme`, `--no-*` | ❌ |
| Other | `--export` (HTML), `--offline`, `--verbose`, `--approve`, `--tui-mode` | ❌ |

## Interactive / TUI

| Capability | Upstream | Rust |
|------------|:--------:|:----:|
| Interactive shell | ✅ full TUI | 🟡 line REPL, plus an opt-in basic TUI (`tui` feature) |
| Fullscreen mode | ✅ | 🟡 with `tui` feature |
| Keybindings / themes | ✅ | ❌ |
| Model cycling (`Ctrl+P`) | ✅ | ❌ |
| Image input | ✅ | ❌ |
| HTML session export | ✅ | ❌ |

## Sessions & context

| Capability | Upstream | Rust |
|------------|:--------:|:----:|
| Persistence | ✅ JSONL session tree | 🟡 single JSON file |
| Resume / continue | ✅ | 🟡 `--resume <id>` only |
| Branching / fork / tree nav | ✅ | ❌ |
| Context compaction | ✅ + branch summarization | 🟡 simple `/compact` |
| Session naming | ✅ | ❌ |

## Extension ecosystem

| Capability | Upstream | Rust |
|------------|:--------:|:----:|
| TypeScript extensions | ✅ | ❌ |
| Skills | ✅ | ❌ |
| Prompt templates | ✅ | ❌ |
| Themes | ✅ | ❌ |
| Pi packages / manager | ✅ | ❌ |
| SDK / RPC mode | ✅ | 🟡 library crates (`pi-ai`, `pi-agent`) |
| MCP client | ✅ | ❌ |

## Configuration & security

| Capability | Upstream | Rust |
|------------|:--------:|:----:|
| Global settings | ✅ `settings.json` | 🟡 `config.toml` (fewer keys) + reads upstream settings |
| Project settings | ✅ | ❌ |
| Credentials | ✅ `auth.json` (+ OAuth) | 🟡 reads upstream `auth.json` keys |
| Custom providers | ✅ `models.json` | 🟡 reads upstream `models.json` |
| Project trust | ✅ `trust.json` | ❌ |
| Sandbox / containerization | ✅ | ❌ (per-tool prompts + `--yolo`) |

## Rust-only extras

- `web_fetch` and `todo` built-in tools.
- Windows shell auto-detection (Git Bash → PowerShell → `cmd.exe`) and a
  dedicated `powershell` tool.
- Optional full-screen TUI built on `ratatui` + `crossterm` (`--features tui`),
  including multi-line input, dimmed reasoning, and light Markdown rendering.
- `--yolo` / `--json` print mode conveniences.

## Out of scope (no plans)

- Porting `@earendil-works/pi-tui` verbatim. The opt-in Rust TUI covers a
  basic chat experience; themes, image input, custom keybindings, and the
  session tree are still missing (`ratatui` would be the base).
- `@earendil-works/pi-web-ui` — browser components.
- Full sandbox parity with the upstream sandbox runtime.

See [ROADMAP.md](../../ROADMAP.md) for the items that are planned (MCP, AWS
Bedrock, OAuth, more provider quirks).
