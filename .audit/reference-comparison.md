# extDB3 reference comparison

The installed Windows extDB3 1.031 binary was copied with its runtime dependency into `.audit/mysql8-reference` and run against a disposable MySQL 8.0.46 container. The replacement was exercised on MySQL 9.7.2 with both final targets. The MySQL 9.7.2 test account uses `caching_sha2_password`, requires SSL, and the server requires secure transport.

The 23-call SQL_CUSTOM probe fixture matches extDB3 on Windows and Linux for every response string. The differential check normalizes process-local ticket values and collapses adjacent `[3]` observations, then compares pending/ready states, final results, multipart markers, fragment counts, and terminators. This includes error formatting, multi-statement ordering, insert IDs, and sync/async multipart handling. It also covers `DATETIME` with `OUTPUT -STRING` and floating-point values, which extDB3 formats as an SQF date array and six decimal places respectively.

Captured reference responses are saved in `.audit/reference-replay.json` for the probe fixture and `.audit/exile-reference-replay.json` for the Exile fixture.

The 15-call Exile fixture also matches all reference response strings byte-for-byte on both replacement targets. It covers the bootstrap, logging and lock calls, vehicle page/safe queries, the `loadUnitsPage`, `readUnit`, and `loadTerritory` signatures observed in RPTs, and sync/fire-and-forget `updatePlayer` calls with 42 synthetic arguments. The RPT observations establish query names, call type, and argument counts only; they do not contain raw argument values.

The reference-only `sql_custom/exile.ini` was adapted for extDB3 1.031 and the small synthetic rows. The old binary rejected `LONG_BLOB` result columns in the unit and territory queries, so the reference copy uses bounded `CHAR` casts. The vehicle queries also use shorter reference-only projections with those same seeded 28-column row values. The replacement used the active Exile SQL_CUSTOM file unchanged. These adaptations make the formatter and request/response comparison runnable, but do not prove extDB3 1.031 can execute each active Exile query unchanged.

The MySQL 9.7.2 service uses a test CA, and both replacement targets complete SQL replays through TLS. A second valid but unrelated CA causes `ADD_DATABASE` to fail on both targets; the extension maps connection setup failure to `[0,"Database Config Error"]`. The legacy reference could not connect to MySQL 9.7.2 and therefore has no direct parity result on the intended server version.

Remote October 10 RPTs contain eight warning-context Exile signatures: five `loadUnitsPage` calls with two arguments, two `readUnit` calls with one argument, and one `loadTerritory` call with one argument. Those counts match the active INI input mappings. The extDB3 logs contain no raw `Input from Server` payloads, and the RPTs omit argument values. The Exile fixture therefore replays the observed call shapes with synthetic values; no production database export was taken.

Reference runs use the copied DLL and isolated test configuration. No production data or configuration was changed during the differential tests.

The installed Linux reference libraries are 32-bit i386, so Windows x64 extDB3 1.031 is the only usable reference binary in this environment.
