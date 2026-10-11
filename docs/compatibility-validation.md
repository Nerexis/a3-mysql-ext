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

The Rust parity change adds the raw `SQL` protocol, its four documented output modes, SQL_CUSTOM SQF-array input parsing, retry settings, non-prepared substitutions, the documented filters, time conversion, RESET, time offsets, and `[Main] Thread` selection. `Thread = 0` uses all detected logical CPUs. The local extDB3 six-worker result with `Thread = 0` was treated as a reference bug, not a limit to copy.

The new behavior has not been built or run through a test suite in this change. The earlier 23-call SQL_CUSTOM comparison therefore does not validate the added behavior. The existing SQL_CUSTOM test `reject_unknown_options_and_replay` also still treats newly supported settings as invalid and needs maintenance before that suite can provide useful coverage.

The [extDB3 SQL_CUSTOM reference](https://github.com/SteezCram/extDB3/wiki/extDB3---sql_custom.ini) defines `BEGUID` as SteamID-to-BattlEye-GUID conversion. Rust uses the little-endian SteamID64 encoding in the [extDB2 implementation](https://github.com/AsYetUntitled/extDB2/blob/master/src/ext.cpp). The result has not been compared against the extDB3 1.031 DLL. The local extDB3 DLL probe confirmed that `Randomize Config File` renames the config once to `extdb3-conf-XXXXXXXX.ini` and discovers it on a later load. Rust implements that behavior on both targets, but native Linux extDB3 behavior has not been compared. Retry error classification and edge behavior for the legacy quote filters have not been compared against the reference DLL. The implementation also preserves Rust's existing behavior of aggregating rows from all SQL_CUSTOM statements because the documented last-statement behavior has not been confirmed against extDB3 1.031.

RESET waits for each pool's `disconnect` call. A stalled connector can leave RESET pending indefinitely; no bounded close timeout has been verified.
