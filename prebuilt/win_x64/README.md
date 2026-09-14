# Prebuilt binaries — Windows x86_64

Ready-to-run `pi` CLI for 64-bit Windows. No Rust toolchain required.

## Files

| File | Size | Description |
|------|------|-------------|
| `pi-1.2.0-windows-x86_64.zip` | 5.0 MB | Zip archive containing `pi.exe` |

This build has the optional **full-screen TUI enabled** (`--features tui`).

## Usage

Extract the archive, then run:

PowerShell / cmd:

```powershell
Expand-Archive .\prebuilt\win_x64\pi-1.2.0-windows-x86_64.zip -DestinationPath .\pi
.\pi\pi.exe --help
.\pi\pi.exe                       # starts the TUI on an interactive terminal
.\pi\pi.exe --no-tui              # line REPL instead
.\pi\pi.exe -p "List the files in this directory"
```

Git Bash:

```bash
unzip prebuilt/win_x64/pi-1.2.0-windows-x86_64.zip -d pi
./pi/pi.exe --version
```

> The `bash` tool auto-detects a shell: it prefers `bash` from Git for
> Windows, then PowerShell, then `cmd.exe`. Install
> [Git for Windows](https://git-scm.com/download/win) for Unix-compatible
> commands. See [`docs/src/install.md`](../../docs/src/install.md#windows).

The CLI reuses the upstream `pi` config under `~/.pi/agent`
(`settings.json` / `auth.json` / `models.json`), so the model, credentials,
and custom providers you already configured for the TypeScript `pi` apply
here too. Set `RUST_LOG=debug` to print the resolved provider/model/base URL.

## TUI keys

| Key | Action |
|-----|--------|
| `Enter` | send |
| `PageUp` / `PageDown` / `Up` / `Down` | scroll |
| `Ctrl+C` | quit |
| `y` / `a` / `n` | answer a permission prompt |

See [`docs/src/cli/tui.md`](../../docs/src/cli/tui.md).

## Build provenance

| Field | Value |
|-------|-------|
| Version | `pi 1.2.0` |
| Commit | `8da482e` (`feat/windows`) |
| Built on | 2026-09-14 |
| Target | `x86_64-pc-windows-gnu` |
| Features | `tui` |
| Toolchain | rustc 1.98.1 (stable) |
| C compiler | w64devkit GCC 16.2.0 (MinGW-w64) |
| Post-processing | `strip --strip-all`, then zip |
| Uncompressed `pi.exe` | 12,306,432 bytes (~11.7 MiB) |
| SHA-256 (`pi.exe`) | `36a8f7a4b0f926ce098712ead97dacfc38489fa4405ea571d7a10ac384ca8edc` |
| SHA-256 (`.zip`) | `2ef1c32813e6f0d037f72db64667060827cea2268d7134257745f36da820eed4` |

Reproduce with:

```bash
cargo build --release -p pi-coding-agent --features tui
strip --strip-all target/release/pi.exe
```

Without `--features tui` the stripped binary is 11,341,824 bytes (~10.8 MiB).

## Runtime requirements

Only OS-provided DLLs (`KERNEL32`, `msvcrt`, `ws2_32`, `bcrypt`, ...). The
MinGW runtime is linked statically, so no `libgcc_s_seh-1.dll` /
`libwinpthread-1.dll` needs to ship alongside it.
