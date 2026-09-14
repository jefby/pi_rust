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
.\pi\pi.exe -r <id|file.jsonl>    # resume a session (bare -r = most recent)
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
| Commit | `6f89384` (`feat/windows`) |
| Built on | 2026-09-14 |
| Target | `x86_64-pc-windows-gnu` |
| Features | `tui` |
| Toolchain | rustc 1.98.1 (stable) |
| C compiler | w64devkit GCC 16.2.0 (MinGW-w64) |
| Post-processing | `strip --strip-all`, then zip |
| Uncompressed `pi.exe` | 12,434,432 bytes (~11.9 MiB) |
| SHA-256 (`pi.exe`) | `b8f632226d751d27eeaa0af04b03a641534ecc27f8512a8e6121a44364300730` |
| SHA-256 (`.zip`) | `fb1937f189c069051cdb79862f9423811730570ed1d1f73f067499bcfcaeb64e` |

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
