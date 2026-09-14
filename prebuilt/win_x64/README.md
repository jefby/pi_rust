# Prebuilt binaries — Windows x86_64

Ready-to-run `pi` CLI for 64-bit Windows. No Rust toolchain required.

## Files

| File | Size | Description |
|------|------|-------------|
| `pi-1.2.0-windows-x86_64.zip` | 4.9 MB | Zip archive containing `pi.exe` |

## Usage

Extract the archive, then run:

PowerShell / cmd:

```powershell
Expand-Archive .\prebuilt\win_x64\pi-1.2.0-windows-x86_64.zip -DestinationPath .\pi
.\pi\pi.exe --help
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

## Build provenance

| Field | Value |
|-------|-------|
| Version | `pi 1.2.0` |
| Commit | `e0de9dc` (`feat/windows`) |
| Built on | 2026-09-14 |
| Target | `x86_64-pc-windows-gnu` |
| Toolchain | rustc 1.98.1 (stable) |
| C compiler | w64devkit GCC 16.2.0 (MinGW-w64) |
| Post-processing | `strip --strip-all`, then zip |
| Uncompressed `pi.exe` | 11,327,488 bytes (~10.8 MiB) |
| SHA-256 (`pi.exe`) | `1657b1d9644205c3286721f1768bd64c243c0e1f4cafeec89a67f2ff63859383` |
| SHA-256 (`.zip`) | `d40aa92105e0bf176a7ddcbbaef9af128c22f537de41b67ef9717bb89e0779eb` |

Reproduce with:

```bash
cargo build --release -p pi-coding-agent
strip --strip-all target/release/pi.exe
```

## Runtime requirements

Only OS-provided DLLs (`KERNEL32`, `msvcrt`, `ws2_32`, `bcrypt`, ...). The
MinGW runtime is linked statically, so no `libgcc_s_seh-1.dll` /
`libwinpthread-1.dll` needs to ship alongside it.
