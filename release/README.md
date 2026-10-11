# Exile extDB3-compatible release

This release contains the 64-bit Windows and Linux Arma extension for Exile. It targets extDB3 protocol version 1.033 and exports `RVExtension`, `RVExtensionArgs`, and `RVExtensionVersion`.

## Platform files

- `windows-x64/extDB3_x64.dll` for the x86-64 Windows MSVC Arma server.
- `linux-x64/extDB3_x64.so` for the x86-64 Linux GNU Arma server.

Copy only the extension for your server platform into a loaded server mod directory. Keep `extdb3-conf.ini` and the `sql_custom` directory with the extension, or set `EXTDB3_ROOT` to their configuration root. This package excludes database credentials and server-specific SQL configuration.

## Use with Exile

The Exile bootstrap PBO `addons/extdb3.pbo` provides the SQF bridge that loads the extension. If needed, copy that PBO from an existing extDB3 installation into the Rust mod's `addons` directory. Put the Rust mod in `-servermod` and leave the old native extDB3 mod out of that argument. Do not copy the old native DLL or SO.

## TLS and connection settings

The extension uses `mysql_async` and Rustls. It does not require a native MySQL client library. TLS is always required, with certificate and hostname verification enabled by default. A custom CA can be configured with `TLS CA`. Verification overrides are accepted only for literal loopback IPs.

`Reset Connection` defaults to `true`. Setting it to `false` retains MySQL session state and prepared statements across pooled requests; use it only when the configured SQL_CUSTOM calls are safe to reuse across requests.

`[Main] Thread` defaults to `0`, which selects one Tokio worker per detected logical CPU. Values from 2 through 6 set an explicit worker count. `Allow Reset` defaults to `false`; set it to `true` to enable `9:RESET` while the extension is unlocked.

The extension supports both raw `SQL` and `SQL_CUSTOM` protocols. Raw SQL accepts `TEXT`, `TEXT2`, `NULL`, and `TEXT-NULL` output options. SQL_CUSTOM supports prepared and substituted statements, SQF-array inputs, retries, and the documented filters except `BEGUID`. `[Main] Randomize Config File` is not supported.

## Source and license

This project is licensed under LGPL-3.0-only. The full project license is included as `LICENSE`, and third-party dependency licenses and notices are included as `THIRD-PARTY-NOTICES.md`. Download the matching `*-source.zip` from the same release when using or redistributing these binaries; it contains the corresponding source revision and build files.
