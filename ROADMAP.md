# Roadmap

> **Status:** All milestones from 0.2.0 through 1.0.0 have shipped in
> [1.0.0](./CHANGELOG.md#100--2026-05-12). The list below records what was
> delivered and what remains for a future major. New unchecked items are now
> targeted at 1.x / 2.0.
>
> For a feature-by-feature comparison against the TypeScript upstream, see
> [docs/src/parity.md](./docs/src/parity.md).

## Milestone 1 — Provider parity for streaming ✅ (delivered in 1.0.0)

- [x] **SSE parsing for Anthropic Messages** (`stream: true`) — emit
      `text_delta` / `thinking_delta` / `toolcall_delta` as they arrive.
- [x] **SSE parsing for OpenAI Chat Completions** (`stream: true`,
      `stream_options.include_usage: true`).
- [x] Surface model reasoning on OpenAI-compatible providers
      (`reasoning_content` / `reasoning`) as `thinking_delta`.
- [x] Surface `Usage` deltas; aggregate the final usage into the
      `AssistantMessage`.
- [x] Cancellation: thread a `CancellationToken` through the stream so
      callers can cancel mid-response.
- [x] Retry policy with exponential back-off and `Retry-After` honoring.

## Milestone 2 — Coding-agent UX ✅ (delivered in 1.0.0)

- [x] Streaming render in the REPL.
- [x] **Session persistence** under `$XDG_CONFIG_HOME/pi/sessions/<id>.json`;
      `pi --resume <id>` and `pi sessions list / show / delete`.
- [x] **Upstream session interop**: import/export the TypeScript `pi` JSONL
      format (`pi sessions import/export`), and `--resume <path.jsonl>`.
- [x] **`AGENTS.md` / project-prompt loading**.
- [x] Slash commands: `/clear` (as `/reset`), `/cost`, `/tools`, `/sessions`,
      `/resume`, `/session`, `/help`, `/model`, `/quit`, `/exit`.
- [x] **Print-mode JSON output** (`-p --json`) — emit structured events for
      scripting. (delivered in 1.1.0)
- [x] **Config file** at `$XDG_CONFIG_HOME/pi/config.toml` (delivered in 1.2.0).
- [x] `/compact` (auto-summarize context to free room) — delivered in 1.2.0.
- [x] **Optional full-screen TUI** built on `ratatui`/`crossterm`
      (`--features tui`) — chat, streaming, multi-line input, dimmed
      reasoning, light Markdown, permission modal.

## Milestone 3 — Tool ecosystem ✅ (mostly delivered in 1.0.0)

- [x] **Per-call permission prompts** with allow / allow-session / deny.
- [x] **New tools**: `web_fetch`, `todo`.
- [x] Rename `glob` → `find`; add the Windows `powershell` tool.
- [x] **CLI parity**: `--thinking`, `--continue`, `--provider`, `--api-key`,
      `--system-prompt`, `--append-system-prompt`, `--no-context-files`,
      `--list-models`; `SYSTEM.md` / `APPEND_SYSTEM.md` loading.
- [x] **`bash` improvements**: streamed stdout/stderr, persisted cwd (1.2.0).
- [x] **`edit` polish**: unified-diff preview before write (1.2.0).
- [x] **`grep` upgrade**: regex mode, context lines (1.2.0).
- [ ] **MCP (Model Context Protocol) client**. (1.x — major work)

## Milestone 4 — More providers ✅ (mostly delivered in 1.0.0)

- [x] **Google Generative AI / Vertex AI** (Gemini via
      `streamGenerateContent?alt=sse`).
- [x] **OpenAI-compatible passthrough** — `Model::openai_compat(...)` or
      `StreamOptions::base_url` covers OpenRouter, Together, Groq, Cerebras,
      DeepSeek, Fireworks, xAI, etc.
- [x] **OpenAI Responses API** (`openai-responses`) — delivered in 1.2.0.
- [ ] **AWS Bedrock Converse Stream**. (1.x)
- [x] **Prompt cache markers** — Anthropic `cache_control` (1.2.0). OpenRouter
      and OpenAI session-id headers still pending.
- [ ] **OAuth flows** for Copilot, Codex. (1.x)

## Milestone 5 — Reliability and polish ✅ (delivered in 1.0.0)

- [x] **CI**: GitHub Actions matrix (stable + MSRV, macOS + Linux), `cargo
      fmt --check`, `cargo clippy -- -D warnings`, `cargo test`.
- [x] **MSRV**: declared as `1.80` in workspace and CI.
- [x] **Release pipeline**: pre-built binaries for macOS (arm64/x86_64) and
      Linux (gnu) per tag via `release.yml`.
- [x] **Structured tracing** with `#[instrument]` on the agent loop.
- [x] **Typed error model** (`pi_agent::AgentError` enum).
- [x] **Crate publishing** of `pi-ai` and `pi-agent` to crates.io.
- [x] **Documentation site** under `docs/` (mdBook) — delivered in 1.2.0.

## Beyond 1.0 — out of scope (no plans)

- Full parity with `@earendil-works/pi-tui` (themes, image input, custom
  keybindings, session tree). The opt-in Rust TUI on `ratatui` covers the
  basic chat experience; the rest is not planned.
- Porting `@earendil-works/pi-web-ui` — browser components.
- Full sandbox parity with `@anthropic-ai/sandbox-runtime`. The pi 1.0
  approach is per-tool permission prompts plus `--yolo` to bypass.

## Non-goals

- One-to-one type compatibility with the TS types (we are idiomatic Rust,
  not a transliteration).
- Bug-for-bug compat with TS provider quirks. We track upstream behavior
  but only port quirks when they affect real-world model output.

## Contributing

Pick any unchecked item, open an issue with the milestone tag, and submit
a PR. See [CHANGELOG.md](./CHANGELOG.md) for what shipped where.
