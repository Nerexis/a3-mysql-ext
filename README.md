# Exile extDB3-compatible Arma extension

A clean-room, 64-bit Rust extension for running Exile SQL_CUSTOM calls through the extDB3-compatible Arma extension ABI. It implements protocol version 1.033 and uses `mysql_async` with Rustls; it does not need a native MySQL client library.

| Server platform | Build target | Extension file |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `extDB3_x64.dll` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `extDB3_x64.so` |

## Supported behavior

- Arma exports `RVExtension`, `RVExtensionArgs`, and `RVExtensionVersion`.
- Synchronous and asynchronous requests, request IDs, result polling, and multipart results.
- SQL_CUSTOM version 1 fragments and mappings, prepared statements, `STRING`, `BOOL`, and `NULL` values, quote escaping, strip-character modes, insert-ID returns, and ordered multi-statement queries.
- extDB3 system commands for setup and locking, version and time queries, uptime, and `DATEADD` calendar arithmetic.
- The LOG protocol, including per-protocol logs and the default runtime log, stored under `logs/<year>/<month>/<day>/`. Entries use the extDB3 timestamp and OS thread ID prefix; each logger rotates at 100 MiB and keeps three backups. Startup entries preserve the original extDB3 version, platform, and configuration messages, then identify the Rust/Tokio runtime, available logical CPUs, Tokio worker count, and runtime thread total.
- Bounded Tokio request queues and a MySQL connection pool per configured database.
- TLS is required. Certificate and hostname verification are enabled by default; a custom CA can be configured.

Unsupported SQL_CUSTOM settings and raw `$CUSTOM_` substitutions fail configuration loading rather than being interpolated into SQL.

The main runtime log follows the extDB3 dated-folder layout and includes startup and system events such as `Output Size` and `Locked`. SQF-facing failures use the original extDB3 error strings where they apply; the Rust log adds request or system-command context and the underlying database or formatting error without recording command arguments.

## Use with Exile

1. Extract the release package and copy only the extension file for the server platform into a loaded server mod directory.
2. Keep the Exile bootstrap PBO at `addons/extdb3.pbo` in the Rust extension mod. If needed, copy this SQF bridge PBO from an existing extDB3 installation; do not copy its native DLL or SO.
3. Put the Rust extension mod in `-servermod` and leave the old native extDB3 mod out of that argument.
4. Keep `extdb3-conf.ini` and its `sql_custom` directory in the extension configuration root. Set `EXTDB3_ROOT` if they are outside the default search paths.

The package does not contain server credentials or production SQL configuration.

## Configuration

Database sections require `IP`, `Username`, `Password`, and `Database`. The port defaults to 3306. `Max Pool Size` defaults to 16 and is limited to 64. Idle connections are kept for up to 60 seconds before the pool shrinks to zero.

`Reset Connection` defaults to `true`. Setting it to `false` retains MySQL session state and prepared statements when a connection returns to the pool; use that only when the configured SQL_CUSTOM calls are safe to reuse across requests.

`[Log] Flush` defaults to `true`, matching extDB3. Set it to `false` to buffer LOG protocol writes and reduce per-entry flush overhead.

TLS cannot be disabled. Set `TLS CA` when the server certificate uses a CA that is not in the system trust store. Verification overrides are accepted only for a literal loopback IP address; keep TLS required for remote databases.

## Build

The repository pins Rust 1.99.0. Build Windows with the MSVC target and Linux on Ubuntu 24.04:

```powershell
cargo build --locked --release --target x86_64-pc-windows-msvc
```

```bash
cargo build --locked --release --target x86_64-unknown-linux-gnu
```

To run the complete release build from Windows, use `tools/build-release.ps1` with the configured Ubuntu 24.04 WSL distribution. It runs formatting, tests, Clippy, release builds, benchmark compilation, and ABI probes for both targets. It creates the runtime ZIP, matching source ZIP, and SHA-256 files under `dist/release`.

## Verify

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The Docker harness in `tests/mysql9` starts an isolated MySQL 9.7.2 service bound to loopback. It verifies `caching_sha2_password` over TLS, accepts the configured test CA, and rejects an unrelated CA. Start it with `tests/mysql9/setup.ps1`.

After changing Cargo dependencies, regenerate the third-party notice bundle with `tools/generate-third-party-notices.ps1`, then verify it with `tools/generate-third-party-notices.ps1 -Check`. The release builder runs the check before packaging.

Replay fixtures with `tools/replay.py` against a built extension. The fixtures cover Exile call signatures observed in server logs, bootstrap, vehicle page and safe loading, unit and territory queries, and a 42-argument player update. The logs did not retain the original arguments, so those replays use synthetic values and verify call shapes and mappings rather than production payload equality.

The 23-call SQL_CUSTOM probe passes on both targets against MySQL 9.7.2. Its response formatting and normalized async/multipart states match extDB3 1.031 on an isolated MySQL 8.0.46 instance. extDB3 1.031 could not connect to the MySQL 9 test service, so there is no direct reference comparison against that server version. Compatibility validation details and known gaps are in `docs/compatibility-validation.md`.

Performance measurements depend on the mission, PBOs, database snapshot, and server environment. See `benches/performance-review-2026-10-10.md` for the benchmark method and recorded results.

## License

This project is licensed under LGPL-3.0-only. See `LICENSE`. `THIRD-PARTY-NOTICES.md` records the license expressions and upstream license or notice texts for the runtime dependencies of both release targets. The release builder includes those notices and creates a source archive matching the binaries; distribute that source archive or the corresponding source revision with the binaries.
