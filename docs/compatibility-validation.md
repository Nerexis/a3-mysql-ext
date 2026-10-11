# Compatibility validation

This note summarizes the development-time comparison with extDB3 1.031 and the Exile call shapes observed in server RPTs. The test fixtures use synthetic argument values because the logs contain query names, call types, and argument counts, but no raw arguments.

## Request and response flow

The 23-call SQL_CUSTOM probe covered synchronous and asynchronous calls, request polling, multipart output, multi-statement order, insert IDs, and error formatting. The development comparison normalized process-local ticket IDs and repeated `[3]` pending polls before comparing final responses, multipart markers, fragment counts, and terminators. It also checked floating-point output and `DATETIME` values with `OUTPUT -STRING`.

The 15-call Exile fixture covered bootstrap, logging and lock calls, vehicle page and safe queries, and the observed `loadUnitsPage`, `readUnit`, `loadTerritory`, and `updatePlayer` signatures. The `updatePlayer` cases use 42 synthetic arguments. The comparison found matching responses on the Windows and Linux Rust targets, except that `9:VERSION` reports the requested protocol version 1.033.

The reference run used the Windows extDB3 1.031 binary on MySQL 8.0.46. The Rust extension ran on MySQL 9.7.2. The extDB3 SQL_CUSTOM file needed bounded casts for legacy `LONG_BLOB` columns and shorter vehicle projections over the same seeded rows. Those changes let extDB3 produce comparable responses, but they do not prove that extDB3 1.031 can run every active Exile query unchanged.

The reference binary and raw response captures are local test artifacts and are not part of this repository. The repository contains the Rust replay inputs and test tools, but not a standalone automated differential runner against extDB3.

## MySQL and TLS

The MySQL 9.7.2 test account uses `caching_sha2_password`, and the server requires secure transport. Both Rust targets connected with the trusted test CA. A different valid CA caused database registration to fail with `[0,"Database Config Error"]`.

extDB3 1.031 could not connect to MySQL 9.7.2, so the reference comparison does not establish parity on that server version. The available Linux reference libraries are 32-bit i386; the usable reference binary was the Windows x64 build.

## Limits

RPTs contain eight warning-context Exile signatures: five `loadUnitsPage` calls with two arguments, two `readUnit` calls with one argument, and one `loadTerritory` call with one argument. Those shapes match the active SQL_CUSTOM input mappings. The captured logs omit argument values, and no production database export was used.

The comparison was run during development. Repeating the exact differential check requires a compatible extDB3 1.031 binary and the isolated reference database; neither is included here.

## Parity implementation status

The Rust implementation covers the documented SQL, SQL_CUSTOM, LOG, and system-command behavior used by this Exile MySQL setup. It adds the raw `SQL` protocol, its four output modes, SQL_CUSTOM SQF-array parsing, retries, non-prepared substitutions, the documented filters, time conversion, RESET, time offsets, and `[Main] Thread` selection. `Thread = 0` uses all detected logical CPUs. The local extDB3 log showed six workers on a 16-core system. The Rust runtime uses all 16 detected logical CPUs instead of copying that cap.

On 2026-10-11, `tools/build-release.ps1` passed its Windows and Linux checks, including formatting, unit tests, Clippy, release builds, benchmark compilation, ABI probes, and third-party notice validation. The local Windows Arma server loaded the same DLL hash as `dist/extDB3_x64.dll`, connected to database alias `exile`, and reached `ExileServer - Game world initialized!`. The Rust runtime log reported 16 detected logical CPUs and 16 Tokio workers. The RPT search found no SQL parser, database configuration, or database connection errors.

The direct extDB3 1.031 probe returned `[1,[["first"],["second"]]]` for a two-statement SQL_CUSTOM call. Rust keeps that aggregation behavior. The [SQL_CUSTOM wiki](https://github.com/SteezCram/extDB3/wiki/extDB3---sql_custom.ini) says to return rows only from the last SQL line, so this observed binary result differs from the documentation.

`BEGUID` uses the little-endian SteamID64 encoding described by the [extDB2 implementation](https://github.com/AsYetUntitled/extDB2/blob/master/src/ext.cpp), but its edge cases have not been compared against extDB3 1.031. The local Windows probe confirmed that `Randomize Config File` renames the config once to `extdb3-conf-XXXXXXXX.ini` and finds it on the next load. Native Linux randomization behavior has not been compared. Retry failure classes and edge cases for the legacy quote filters also remain unverified against the reference DLL.

RESET waits for every pool's `disconnect` call. A stalled connector can leave RESET pending indefinitely. The reference DLL's close timeout and result-retention behavior have not been established.
