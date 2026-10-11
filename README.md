# Rust extDB3-compatible extension for Exile

This crate builds a 64-bit Arma extension as a Windows DLL and Linux shared object. It implements the extDB3 request flow used by the active Exile SQL_CUSTOM configuration: synchronous and asynchronous queries, request tickets, result polling, multipart output, and the SQL_CUSTOM options exercised by the server's INI. It is a clean-room implementation and does not link a native MySQL client library.

The ABI exports `RVExtension`, `RVExtensionArgs`, and `RVExtensionVersion`. Windows uses the system ABI; Linux uses C ABI. The extension reports protocol version 1.033, accepts Arma's default escaped-string and unescaped-string forms, and accepts the documented maximum of 2,048 `RVExtensionArgs` values. Both targets use bounded Tokio request, job, and completion queues with no fixed active-job cap; Tokio uses its default multi-thread runtime sizing, and each database pool separately limits concurrent MySQL connections. TLS is required, certificate verification and hostname verification remain enabled, and a custom CA can be added for the server certificate.

## Build

Install Rust `1.99.0` with the Windows MSVC target, and install the same toolchain with the GNU target in Ubuntu 24.04 under WSL. The lockfile pins the dependency graph.

From PowerShell at the repository root:

```powershell
cargo build --locked --release --target x86_64-pc-windows-msvc
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\x86_64-pc-windows-msvc\release\extDB3_x64.dll dist\extDB3_x64.dll
```

Build the Linux artifact from Ubuntu 24.04 under WSL. Set `REPO_WSL` to this checkout's WSL path:

```bash
cd "$REPO_WSL"
CARGO_TARGET_DIR=/tmp/extdb3-linux-target cargo build --locked --release --target x86_64-unknown-linux-gnu
cp /tmp/extdb3-linux-target/x86_64-unknown-linux-gnu/release/libextDB3_x64.so "$REPO_WSL/dist/extDB3_x64.so"
```

The distributable filenames are `extDB3_x64.dll` and `extDB3_x64.so`.

For the release build checks and package, run `tools/build-release.ps1` from
PowerShell with the configured Ubuntu 24.04 WSL distribution. It tests, lints,
builds, and ABI-probes both targets, then writes the binaries, manifest, README,
ZIP, and SHA-256 file under `dist/release/`.

## Runtime configuration

The extension reads `extdb3-conf.ini` from `EXTDB3_ROOT` when set. Otherwise it checks the current directory, `@extdb3`, `@extDB3`, `mods/@extdb3`, and `mods/@extDB3`. SQL_CUSTOM files are loaded from the `sql_custom` subdirectory. Database sections require `IP`, `Username`, `Password`, and `Database`; `Port` defaults to 3306 and `Max Pool Size` defaults to 16 (maximum 64). Idle connections are retained for up to 60 seconds before the pool shrinks back to zero. `Reset Connection` defaults to `true`; setting it to `false` retains MySQL session state and prepared statements when a connection returns to the pool, which can reduce repeated-query latency. Use `false` only when the configured SQL_CUSTOM calls are safe to reuse across requests.

TLS cannot be disabled. Set `TLS CA` to a CA certificate path relative to the configuration root when the system trust store does not contain the issuing CA. `TLS Verify` and `TLS Verify Hostname` default to `true`; setting either to `false` skips that check only for literal loopback IPs. Keep `TLS Required = true` so traffic remains encrypted. This exception is intended for local testing, not remote databases.

SQL_CUSTOM accepts version 1, numbered SQL fragments and input mappings, prepared statements, output mappings, `STRING`, `BOOL`, `NULL`, `add_escape_quotes`, strip character modes, insert-ID returns, and ordered multi-statement queries. Unsupported settings and raw `$CUSTOM_` substitutions fail protocol loading instead of being interpolated into SQL.

## Verification

Run the Rust checks from PowerShell:

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

To include the server's active SQL_CUSTOM INI parser check, set `SQL_CUSTOM_TEST_CONFIG` to the local `exile.ini` path before `cargo test --locked`.

The isolated MySQL 9.7.2 test service uses the official `mysql:9.7.2` image, binds only to `127.0.0.1:33307`, and keeps its data in a named Docker volume. The setup script creates local certificates, copies a test-only configuration and the probe SQL_CUSTOM file, and starts the service. Start it with:

```powershell
.\tests\mysql9\setup.ps1
```

The service requires encrypted connections. Its test credentials and generated files stay under `tests/mysql9` and are ignored by Git. From `tests/mysql9`, replay the probe fixture against the Windows DLL:

```powershell
python ..\..\tools\replay.py ..\..\dist\extDB3_x64.dll ..\..\tests\fixtures\probe-calls.json --output "$env:TEMP\extdb3-probe-replay.json"
```

The same harness can load the Linux `.so` from WSL. ABI-only probes use `tools/abi_probe.py`; the release script also probes both packaged libraries.

For a Windows request-stage profile, build an instrumented release into a separate target directory and set `EXTDB3_PROFILE_PATH` before launching Arma:

```powershell
$env:CARGO_TARGET_DIR = 'target\profile'
cargo build --locked --release --features profile-requests --target x86_64-pc-windows-msvc
$env:EXTDB3_PROFILE_PATH = "$PWD\.audit\request-profile.csv"
```

Deploy that build only for the profiling run, launch the server from a process that inherits the environment, then summarize the CSV with `python tools\summarize_profile.py .audit\request-profile.csv`. It records queue, preparation, pool-acquisition, MySQL async, formatting, and completion times by query name without SQL text or arguments. The instrumented build adds timing and file-write overhead; use its profile to find a stage, then verify any optimization with the normal release build.

To check that neither target has a fixed active-job cap, copy `tests/fixtures/worker-probe.ini` into the configured `sql_custom` directory, set the test section's `Max Pool Size` to at least 24, and run `tools/worker_probe.py` against the target library. It runs 24 concurrent read-only `SELECT SLEEP` calls and fails if they take longer than one sleep interval plus the configured margin.

The TLS fixture uses the test CA for one database section and a different CA for another; it expects the wrong CA to fail and the trusted CA to connect:

```powershell
python ..\..\tools\replay.py ..\..\dist\extDB3_x64.dll ..\..\tests\fixtures\tls-calls.json --output tls-replay.json
```

## Compatibility status

The Exile replay fixtures cover bootstrap, vehicle page/safe loading, unit and territory query signatures observed in server logs, and the 42-input player update. Logs do not retain the observed calls' argument values, so those cases use synthetic inputs. This verifies call shapes and mappings, not exact runtime payloads.

A separate AVS fixture covers vehicle-ammo get/set query signatures with a synthetic, nonmatching ID. It passes on Windows and Linux against the isolated MySQL clone without changing a row.

The 23-call SQL_CUSTOM probe fixture passes on Windows and Linux against MySQL 9.7.2, and its SQL responses and normalized async/multipart polling states match extDB3 1.031 on isolated MySQL 8.0.46. The Exile response fixture also matches except for `9:VERSION`, which now reports the targeted 1.033 protocol version. The reference-only config uses bounded casts for legacy `LONG_BLOB` fields and a shorter vehicle projection over the same small synthetic rows; it verifies response formatting and call handling, not whether extDB3 1.031 can execute every active Exile query unchanged. The comparison caught and fixed extDB3 formatting differences: floating-point values use six decimal places, and SQL `DATETIME` results remain SQF date arrays even with `OUTPUT -STRING`.

MySQL 9.7.2 reports the test account's plugin as `caching_sha2_password` and requires SSL. Both replacement targets connect using the trusted test CA; a valid unrelated CA fails during database registration. extDB3 1.031 cannot connect to that MySQL 9 service, so direct reference comparison on MySQL 9.7.2 remains unavailable. The remaining payload gap and reference limitations are recorded in [compatibility validation](docs/compatibility-validation.md).

The upstream archive [lists v1.033.1 as its latest release](https://github.com/SteezCram/extDB3/releases/tag/v1033.1). Its Windows release notes add `RVExtensionVersion` and `RVExtensionArgs`; this implementation provides those interfaces on both targets and follows Arma's [2,048-argument limit](https://community.bistudio.com/wiki/callExtension). The upstream Windows release also removes its worker-thread limit. This implementation has no fixed active-job cap on either target, uses Tokio's default worker-thread sizing, and retains bounded request queues plus per-database connection limits. A 24-call read-only worker probe completed in about 2.04 seconds on each target using the MySQL 9.7.2 test clone and a 32-connection pool.

Synthetic scaling measurements and the live startup comparison, including their limitations, are documented in [the performance review](benches/performance-review-2026-10-10.md). The development-time extDB3 comparison and its known limits are recorded in [compatibility validation](docs/compatibility-validation.md).

The latest snapshot-restored comparison used the v0.1.0 Rust release DLL with default connection reset. Native extDB3 1.031 measured a 9.626 s median `WorldInitialize` (9.608–9.820 s). Rust measured 9.723 s (9.715–10.323 s). The paired Rust-minus-native median was +0.115 s, and the exact signed-rank test did not distinguish the backends (p = 0.3125). Rust remains 0.723 s above the under-9-second target. See the [performance review](benches/performance-review-2026-10-10.md) for the test setup, paired timings, and limitations.
