# extDB3-compatible release

This package contains the 64-bit Arma extension for Windows and Linux. It targets extDB3 protocol version 1.033 and exports `RVExtension`, `RVExtensionArgs`, and `RVExtensionVersion`.

## Platform files

- `windows-x64/extDB3_x64.dll` for the x86-64 Windows MSVC Arma server.
- `linux-x64/extDB3_x64.so` for the x86-64 Linux GNU Arma server.

Copy only the file for the server platform into a loaded server mod directory. Keep the server's `extdb3-conf.ini` and `sql_custom` directory with that extension, or set `EXTDB3_ROOT` to the directory containing them. The package intentionally excludes database credentials and server-specific SQL configuration.

## Use with Exile

For this Exile setup, `addons/extdb3.pbo` provides the SQF preInit that loads the extension. Copy that PBO from an existing extDB3 installation into the Rust mod's `addons` directory. Put the Rust mod in `-servermod` and leave the old extDB3 mod out of that argument. Do not copy the old native DLL or SO.

The extension uses `mysql_async` and Rustls. It does not require a native MySQL client DLL or shared library. TLS is always required. Certificate and hostname verification are enabled by default; `TLS Verify = false` and `TLS Verify Hostname = false` can be used only with a literal loopback IP for local testing.

The latest strict alternating startup comparison restored the same SQL snapshot before each of five starts per backend. Rust measured a 9.835-second median `WorldInitialize` (9.779–9.937 seconds), versus 9.666 seconds (9.625–9.986 seconds) for native extDB3 1.031. The observed Rust median was 0.169 seconds (1.75%) slower, but the ranges overlap and an exact paired signed-rank test is inconclusive (p = 0.44). Every run logged 1,017 SQL calls and 581 successful results, with no empty results or database/protocol errors. Rust still misses the under-9-second target. These figures are a local comparison, not a performance guarantee; the method and limitations are documented in the source repository's performance review.
