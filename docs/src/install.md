# Install

Install the CLI from crates.io:

```bash
cargo install pi-coding-agent
```

The crate name is `pi-coding-agent`; the installed binary is `pi`.

## Windows

`pi` builds and runs on Windows with either the MSVC or GNU Rust toolchain
(`x86_64-pc-windows-msvc` / `x86_64-pc-windows-gnu`). Building from source
needs a C compiler because the TLS stack (`ring`) compiles C/assembly:

- MSVC: install **Visual Studio Build Tools** (the `cl.exe` C++ toolchain).
- GNU: install **MinGW-w64** and add `gcc` to `PATH`.

```powershell
cargo install pi-coding-agent
```

The `bash` tool picks the best available shell automatically, in this order:

1. `bash` on `PATH`, or Git for Windows at the usual install paths
   (`%ProgramFiles%\Git\bin\bash.exe`). Recommended — commands behave the
   same as on Unix.
2. PowerShell 7 (`pwsh`) or Windows PowerShell.
3. `cmd.exe` (always present).

For the most predictable behavior, install [Git for Windows](https://git-scm.com/download/win)
so the tool can use `bash`.

## Environment variables

`pi` reads its API key from the environment based on which model you target:

| Variable | Provider |
|----------|----------|
| `ANTHROPIC_API_KEY` | Anthropic Messages (Claude) |
| `OPENAI_API_KEY` | OpenAI Chat Completions, and any OpenAI-compatible endpoint (OpenRouter, Groq, Together, Cerebras, DeepSeek, Fireworks, xAI, ...) |
| `GOOGLE_API_KEY` or `GEMINI_API_KEY` | Google Generative AI (Gemini) |

```bash
export ANTHROPIC_API_KEY=sk-ant-...
pi -p "Say hi"
```

You can also pick the active model explicitly with `PI_MODEL`:

```bash
PI_MODEL=claude-opus-4-7   pi -p "..."   # Anthropic
PI_MODEL=gpt-4o            pi -p "..."   # OpenAI
PI_MODEL=gemini-2.0-flash  pi -p "..."   # Google
```

## Reusing the upstream `pi` configuration

The CLI reads the TypeScript `pi` agent directory (`~/.pi/agent`, overridable
with `PI_CODING_AGENT_DIR`) so both share model selection and credentials:

| File | Used for |
|------|----------|
| `settings.json` | `defaultProvider`, `defaultModel`, `defaultThinkingLevel` |
| `auth.json` | per-provider API keys (`{"type":"api_key","key":"..."}`) |
| `models.json` | custom providers (`baseUrl`, `api`, `apiKey`, `models`) |

The active model is resolved in this order:

1. `-m` / `PI_MODEL` / `config.toml` naming a built-in alias
2. a model declared by a custom provider in `models.json`
3. the upstream `settings.json` default provider + model
4. the environment-key fallback described above

Provider `api` + base URL come from `models.json` when present, otherwise from a
built-in table (`anthropic`, `openai`, `google`, `deepseek`, `openrouter`,
`groq`, `xai`, `mistral`, `together`, `fireworks`, `cerebras`, `moonshotai`,
`moonshotai-cn`, `kimi-coding`). With a stock upstream install, running `pi`
with no flags picks up e.g. `deepseek/deepseek-v4-flash` and the matching key
from `auth.json` with no extra setup. Set `RUST_LOG=debug` to print the
resolved provider, model, base URL, and whether a key was found.

The **model catalog** is read too — `models-store.json` plus the bundled
`pi-ai` provider data (located via `PATH`) — so `contextWindow` and
`maxTokens` are accurate (e.g. 1,000,000 for `deepseek-v4-flash`). This drives
the TUI status bar's context usage.

## Full-screen TUI

An optional full-screen TUI (conversation pane, streaming output, permission
modal) is available behind the `tui` cargo feature:

```bash
cargo install pi-coding-agent --features tui
# or
cargo build --release -p pi-coding-agent --features tui
```

On an interactive terminal `pi` starts the TUI automatically; `--no-tui`
forces the line REPL. The feature is off by default and adds roughly 0.9 MiB
to the stripped release binary. See [TUI](./cli/tui.md) for keys and details.
