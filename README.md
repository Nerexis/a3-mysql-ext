# Exile extDB3-compatible Arma extension

[![Windows build](https://img.shields.io/github/check-runs/Nerexis/a3-mysql-ext/master?nameFilter=native-and-mysql%20%2F%20windows-x64&logo=windows&label=Windows)](https://github.com/Nerexis/a3-mysql-ext/actions/workflows/ci.yml) [![Linux build](https://img.shields.io/github/check-runs/Nerexis/a3-mysql-ext/master?nameFilter=native-and-mysql%20%2F%20linux-x64&logo=linux&label=Linux)](https://github.com/Nerexis/a3-mysql-ext/actions/workflows/ci.yml)

An independently developed 64-bit Rust extension that implements the extDB3-compatible Arma extension ABI for raw SQL and SQL_CUSTOM calls. No code from unpublished extDB3 DLL or SO builds was used. It targets protocol version 1.033 and uses `mysql_async` with Rustls; it does not need a native MySQL client library.

| Server platform | Build target | Extension file |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `extDB3_x64.dll` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `extDB3_x64.so` |

## Supported behavior

- Arma exports `RVExtension`, `RVExtensionArgs`, and `RVExtensionVersion`.
- Synchronous and asynchronous requests, request IDs, result polling, and multipart results.
- Raw `SQL` protocol calls with the `TEXT`, `TEXT2`, `NULL`, and `TEXT-NULL` result options.
- SQL_CUSTOM version 1 fragments and mappings, prepared statements, `$CUSTOM_n$` substitutions, SQF-array inputs, retries, insert-ID returns, and ordered multi-statement queries.
- SQL_CUSTOM `BEGUID`, `STRING`, `STRING2`, `BOOL`, `NULL`, `TIME`, strip, quote, and `mysql_escape` filters.
- extDB3 system commands for setup and locking, version and time queries, `RESET`, uptime, and `DATEADD` calendar arithmetic. `LOCAL_TIME` and `UTC_TIME` accept hour or date-array offsets.
- The LOG protocol, including per-protocol logs and the default runtime log, stored under `logs/<year>/<month>/<day>/`. Entries use the extDB3 timestamp and OS thread ID prefix; each logger rotates at 100 MiB and keeps three backups. Startup entries preserve the original extDB3 version, platform, and configuration messages, then identify the Rust/Tokio runtime, configured thread setting, detected logical CPUs, and selected Tokio worker count.
- Bounded Tokio request queues and a MySQL connection pool per configured database.
- TLS is required. Certificate and hostname verification are enabled by default; a custom CA can be configured.

`Prepared Statement=true` is the default. Set it to `false` to use `$CUSTOM_n$` substitutions. Use the `mysql_escape` input filter with substitutions. `Number of Retrys` defaults to `5`.

The main runtime log follows the extDB3 dated-folder layout and includes startup and system events such as `Output Size` and `Locked`. SQF-facing failures use the original extDB3 error strings where they apply; the Rust log adds request or system-command context and the underlying database or formatting error without recording command arguments.

## Use with Exile

1. Extract the release package and copy only the extension file for the server platform into a loaded server mod directory.
2. Keep the Exile bootstrap PBO at `addons/extdb3.pbo` in the Rust extension mod. If needed, copy this SQF bridge PBO from an existing extDB3 installation; do not copy its native DLL or SO.
3. Put the Rust extension mod in `-servermod` and leave the old native extDB3 mod out of that argument.
4. Keep `extdb3-conf.ini` and its `sql_custom` directory in the extension configuration root. Set `EXTDB3_ROOT` if they are outside the default search paths.

The package does not contain server credentials or production SQL configuration.

## Configuration

Database sections require `IP`, `Username`, `Password`, and `Database`. The port defaults to 3306. `Max Pool Size` defaults to 16 and is limited to 64. Idle connections are kept for up to 60 seconds before the pool shrinks to zero.

`[Main] Thread` defaults to `0`. In automatic mode, the runtime selects one Tokio worker for every detected logical CPU. Values from 2 through 6 set an explicit worker count. The startup log reports the configured value, detected CPUs, and selected workers. `Allow Reset` defaults to `false`; set it to `true` to enable `9:RESET`. RESET requires an unlocked extension, drains accepted work, and closes database pools before replying. `Randomize Config File` defaults to `false`. Set it to `true` to rename `extdb3-conf.ini` once to `extdb3-conf-XXXXXXXX.ini`; the runtime discovers the renamed file on later starts.

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

To run the complete release build from Windows, use `tools/build-release.ps1` with the configured Ubuntu 24.04 WSL distribution. It runs formatting, tests, Clippy, release builds, benchmark compilation, and ABI probes for both targets, then validates the native artifact receipts and packages the runtime ZIP, matching source ZIP, and SHA-256 files under `dist/release`. The same per-target checks and packager are used by GitHub Actions.

GitHub Actions runs the native checks for pull requests and pushes to `master`. A MySQL 9.7.2 job replays the SQL, TLS, and synthetic Exile fixtures against the Linux artifact. Pushing a `vMAJOR.MINOR.PATCH` tag that matches `Cargo.toml` builds the same targets and creates or refreshes a draft GitHub Release with the runtime and source archives. Review and publish the draft separately.

The native artifact receipts bind each uploaded file to its target, source commit, package version, Rust compiler version, and SHA-256. They detect mismatched or incomplete workflow artifacts; they do not provide cryptographic provenance or replace GitHub's workflow and artifact security controls. The release workflow uses read-only repository access through validation and packaging, and grants release-write access only to the draft creation job. It does not use production credentials or database configuration. Workflow action pins link to their official source repositories: [checkout](https://github.com/actions/checkout/tree/3d3c42e5aac5ba805825da76410c181273ba90b1), [cache](https://github.com/actions/cache/tree/55cc8345863c7cc4c66a329aec7e433d2d1c52a9), [upload-artifact](https://github.com/actions/upload-artifact/tree/cf430e030ddbb5b0abf93d22962f4752f3646cd9), and [download-artifact](https://github.com/actions/download-artifact/tree/9000827ccba6bdab643e8b6fd33ac0654aef8333).

## Verify

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The Docker harness in `tests/mysql9` starts an isolated MySQL 9.7.2 service bound to loopback. It verifies `caching_sha2_password` over TLS, accepts the configured test CA, rejects an unrelated CA, and supplies deterministic rows for the SQL_CUSTOM call-shape fixtures. Start it with `tests/mysql9/setup.ps1` on Windows or `bash tests/mysql9/setup.sh` on Linux. Both copy the checked-in synthetic `tests/fixtures/exile.ini`; it contains test queries only and no production SQL. The SQL schema runs only when MySQL initializes a new data volume. Existing local volumes need a manual reset to pick up schema changes; setup does not remove them.

After changing Cargo dependencies, regenerate the third-party notice bundle with `tools/generate-third-party-notices.ps1`, then verify it with `tools/generate-third-party-notices.ps1 -Check`. The release builder runs the check before packaging.

Replay fixtures with `tools/replay.py` against a built extension. The fixtures cover Exile call signatures observed in server logs, bootstrap, vehicle page and safe loading, unit and territory queries, and a 42-argument player update. The logs did not retain the original arguments, so those replays use synthetic values and verify call shapes and mappings rather than production payload equality.

The parity changes passed the Windows and Linux release checks on 2026-10-11. A local Windows Arma server loaded the release DLL, connected to the Exile database, and initialized the game world. Its runtime log reports 16 detected logical CPUs and 16 Tokio workers for `Thread = 0`. These checks do not resolve every undocumented behavior of the original DLL. See `docs/compatibility-validation.md` for the comparison limits.

Performance measurements depend on the mission, PBOs, database snapshot, and server environment. See `benches/performance-review-2026-10-10.md` for the benchmark method and recorded results.

## License

This project is licensed under LGPL-3.0-only. See `LICENSE`. `THIRD-PARTY-NOTICES.md` records the license expressions and upstream license or notice texts for the runtime dependencies of both release targets. The release builder includes those notices and creates a source archive matching the binaries; distribute that source archive or the corresponding source revision with the binaries.
