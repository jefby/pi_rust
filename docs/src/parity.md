# Parity with upstream `pi`

How the Rust port (`pi-coding-agent` 1.2.0) compares with the TypeScript
upstream it is ported from.

> **Snapshot:** upstream `@earendil-works/pi-coding-agent` **0.85.1** vs this
> workspace at **1.2.0** (`feat/windows`, commit `7f40c0d`). Recorded
> 2026-09-14. Upstream moves quickly; treat this as a point-in-time gap
> analysis, not a contract.

**Bottom line:** the Rust port now covers the core coding-agent loop, an
opt-in full-screen TUI, tree sessions with upstream interop, and a broad set
of OpenAI/Anthropic/Google-compatible providers. It is still **not** a full
feature equivalent of 0.85.1 — the extension ecosystem, OAuth/subscription
login, and several provider APIs remain unported. Roughly 9k lines of Rust
`src` against ~189k lines of TypeScript.

Legend: ✅ parity · 🟡 partial · ❌ missing · ➕ Rust-only extra

## Built-in tools

| Tool | Upstream 0.85.1 | Rust 1.2.0 |
|------|:---------------:|:----------:|
| `read` | ✅ (incl. images) | 🟡 text only (no image input) |
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
| Streaming reasoning (`thinking_delta`) | ✅ | ✅ Anthropic thinking + OpenAI-compatible `reasoning_content` / `reasoning` |
| OAuth / subscription login (`/login`) | ✅ | ❌ |
| Provider-specific quirks (cache markers, adaptive thinking, etc.) | ✅ | 🟡 partial (Anthropic cache markers; DeepSeek/OpenRouter reasoning) |

The [upstream-config bridge](./install.md#reusing-the-upstream-pi-configuration)
maps a provider name to `api` + base URL and reuses the configured key, so an
OpenAI-compatible or Anthropic-compatible provider (DeepSeek, OpenRouter, Groq,
Kimi, ...) works. It also reads the upstream model catalog
(`models-store.json` + the bundled `pi-ai` provider data) for accurate
`contextWindow` / `maxTokens`. It does **not** add provider-specific behavior
beyond that.

## CLI surface

| | Upstream 0.85.1 | Rust 1.2.0 |
|---|-----------------|------------|
| Subcommands | `install`, `remove`, `uninstall`, `update`, `list`, `config`, `auth` | `sessions list/show/delete/import/export` |
| Output modes | `text`, `json`, `rpc` | text, `--json` (print mode) |
| Options | ~40 | ~20 (see [Options](./cli/options.md)) |
| Provider/model selection | `--provider`, `--model <pattern>` (glob/`provider/id`), `--models`, `--list-models` | `-m` / `--provider` / `--model` / `--list-models`; reasoning via `--thinking` |
| Tools control | `--tools`, `--exclude-tools`, `--no-tools`, `--no-builtin-tools` | ❌ |
| System prompt | `--system-prompt`, `--append-system-prompt` | ✅ |
| Sessions | `--continue`, `--resume`, `--session`, `--session-id`, `--fork`, `--session-dir`, `--no-session`, `--name` | 🟡 `-r`/`--resume`, `--session`, `--continue`, `--fork`; upstream `.jsonl` tree import/export |
| Resources | `--extension`, `--skill`, `--prompt-template`, `--theme`, `--no-*` | ❌ |
| Other | `--export` (HTML), `--offline`, `--verbose`, `--approve`, `--tui-mode` | ❌ |

## Interactive / TUI

| Capability | Upstream | Rust |
|------------|:--------:|:----:|
| Interactive shell | ✅ full TUI | ✅ line REPL + opt-in full-screen TUI (`--features tui`) |
| Fullscreen mode | ✅ | ✅ with `tui` feature |
| Multi-line input | ✅ | ✅ `Shift+Enter` |
| Reasoning display | ✅ | ✅ dimmed thinking pane |
| Markdown rendering | ✅ | 🟡 light (code fences, headings, bold, inline code) |
| Status bar | ✅ pwd / tokens / context / model | ✅ two-line footer (pwd + git branch; tokens/cost/context and model • thinking) |
| Session picker / tree overlay | ✅ | ✅ `-r` picker, `/tree` (foldable), `/fork` |
| CJK / wide-char input | ✅ | ✅ display-width cursor + wrapping |
| Keybindings / themes | ✅ | ❌ |
| Model cycling (`Ctrl+P`) | ✅ | ❌ |
| Image input | ✅ | ❌ |
| HTML session export | ✅ | ❌ |

## Sessions & context

| Capability | Upstream | Rust |
|------------|:--------:|:----:|
| Persistence | ✅ JSONL session tree | 🟡 single JSON file holding the tree (`entries`/`parent_id`/`active_leaf`); legacy flat files migrate on load |
| Upstream `.jsonl` interop | ✅ | ✅ lossless tree import/export (incl. `model_change`/`custom` entries); `pi sessions import/export` |
| Resume / continue | ✅ | ✅ `-r`/`--resume`/`--session`/`--continue`, picker for bare `-r` |
| New session | ✅ | ✅ `/new` (alias `/reset`) |
| Branching / fork / tree nav | ✅ | ✅ in-file `/tree` (foldable, branch summary), `/fork`, `/clone`, `--fork` |
| Context compaction | ✅ + branch summarization | 🟡 `/compact`; branch summarization on `/tree` switch |
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
| Model catalog (context window, limits) | ✅ | ✅ `models-store.json` + bundled `pi-ai` provider data |
| Project trust | ✅ `trust.json` | ❌ |
| Sandbox / containerization | ✅ | ❌ (per-tool prompts + `--yolo`) |

## Rust-only extras

- `web_fetch` and `todo` built-in tools.
- Windows shell auto-detection (Git Bash → PowerShell → `cmd.exe`) and a
  dedicated `powershell` tool.
- Optional full-screen TUI built on `ratatui` + `crossterm` (`--features tui`):
  multi-line input, dimmed reasoning, light Markdown, CJK-safe input, a
  two-line status bar, session picker and foldable tree overlay.
- `--yolo` / `--json` print mode conveniences; `--tui` / `--no-tui` to force
  the mode.

## Out of scope (no plans)

- Full parity with `@earendil-works/pi-tui`: themes, image input, custom
  keybindings, `Ctrl+P` model cycling. The opt-in Rust TUI covers the core
  chat, tree, and status experience; the rest is not planned.
- Porting `@earendil-works/pi-web-ui` — browser components.
- Full sandbox parity with the upstream sandbox runtime.
- The TypeScript extension/skill/prompt-template/theme package runtime.

See [ROADMAP.md](../../ROADMAP.md) for items that are planned (MCP, AWS
Bedrock, OAuth, `--tools`/`--exclude-tools`, more provider quirks).
