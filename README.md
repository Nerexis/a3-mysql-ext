# Rust extDB3-compatible extension for Exile

This crate builds a 64-bit Arma extension as a Windows DLL and Linux shared object. It implements the extDB3 request flow used by the active Exile SQL_CUSTOM configuration: synchronous and asynchronous queries, request tickets, result polling, multipart output, and the SQL_CUSTOM options exercised by the server's INI. It is a clean-room implementation and does not link a native MySQL client library.

The ABI exports `RVExtension`, `RVExtensionArgs`, and `RVExtensionVersion`. Windows uses the system ABI; Linux uses C ABI. Database work runs through a bounded Tokio queue with a connection pool per configured database. TLS is required, certificate verification and hostname verification remain enabled, and a custom CA can be added for the server certificate.

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

## Runtime configuration

The extension reads `extdb3-conf.ini` from `EXTDB3_ROOT` when set. Otherwise it checks the current directory, `@extdb3`, `@extDB3`, `mods/@extdb3`, and `mods/@extDB3`. SQL_CUSTOM files are loaded from the `sql_custom` subdirectory. Database sections require `IP`, `Username`, `Password`, and `Database`; `Port` defaults to 3306 and `Max Pool Size` defaults to 16 (maximum 64).

TLS cannot be disabled. Set `TLS CA` to a CA certificate path relative to the configuration root when the system trust store does not contain the issuing CA. `TLS Verify`, `TLS Verify Hostname`, and `TLS Required` may be omitted or set to `true`; false values are rejected.

SQL_CUSTOM accepts version 1, numbered SQL fragments and input mappings, prepared statements, output mappings, `STRING`, `BOOL`, `NULL`, `add_escape_quotes`, strip character modes, insert-ID returns, and ordered multi-statement queries. Unsupported settings and raw `$CUSTOM_` substitutions fail protocol loading instead of being interpolated into SQL.

## Verification

Run the Rust checks from PowerShell:

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

To include the server's active SQL_CUSTOM INI parser check, set `SQL_CUSTOM_TEST_CONFIG` to the local `exile.ini` path before `cargo test --locked`.

The isolated MySQL 9.7.2 test service uses the official `mysql:9.7.2` image, binds only to `127.0.0.1:33307`, and keeps its data in a named Docker volume. Start it with:

```powershell
.audit\mysql9\setup.ps1
```

This creates a test CA and starts MySQL with secure transport required. The disposable test credentials and schema live in `.audit/mysql9`; do not use them outside this test environment. From `.audit/mysql9`, copy the probe SQL_CUSTOM fixture and replay it against the Windows DLL:

```powershell
Copy-Item ..\..\tests\fixtures\probe.ini sql_custom\probe.ini -Force
python ..\..\tools\replay.py ..\..\dist\extDB3_x64.dll ..\..\tests\fixtures\probe-calls.json --output replay.json
```

The same harness can load the Linux `.so` from WSL. ABI-only probes use `tools/abi_probe.py`; the results recorded during this run are under `.audit`.

The TLS fixture uses the test CA for one database section and a different CA for another; it expects the wrong CA to fail and the trusted CA to connect:

```powershell
python ..\..\tools\replay.py ..\..\dist\extDB3_x64.dll ..\..\tests\fixtures\tls-calls.json --output tls-replay.json
```

## Compatibility status

The active Exile `exile.ini` was parsed and exercised with source-derived calls for server bootstrap, vehicle page/safe loading, and the 42-input player update. The SQL write was verified in the isolated database. Those arguments were synthetic and follow the observed SQF payload order; no runtime-captured production call log was available.

MySQL 9.7.2 authenticated with `caching_sha2_password` over verified TLS on both native builds, and both builds rejected an unrelated CA. Twenty SQL_CUSTOM fixture calls passed on Windows and Linux, covering result formatting, errors, async polling, and synchronous/async multipart output. Sixteen overlapping responses match the saved extDB3 1.031 replay byte-for-byte.

The extDB3 1.031 reference binary returned `Database Config Error` when connecting to MySQL 9.7.2, so direct reference comparison against that server is incomplete. The saved 16-response comparison is from an earlier MySQL 8 run, and the archived reference fixture did not produce a multipart result. These are compatibility gaps, not passed acceptance checks; see `.audit/reference-comparison.md`.
