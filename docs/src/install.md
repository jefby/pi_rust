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
