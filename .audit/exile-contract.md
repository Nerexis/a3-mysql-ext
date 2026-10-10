# Exile / extDB3 consumer contract

Scope: read-only inspection of the active server addon extraction, SQL_CUSTOM INIs, extDB3 reference binaries, RPT/log metadata, and project documentation. The isolated test configuration was used for replays; no production extDB3 connection config, database credentials, or database export was inspected or taken. No extDB3 implementation source was inspected.

## Call flow

The active consumer extraction's database bootstrap, `ExileServer_system_database_connect.sqf`, calls the extension `extDB3` in this order:

1. `9:VERSION`, then rejects a numeric result below `1.027`.
2. `9:ADD_DATABASE:exile` (connection details are delegated to the extension's external configuration; values were not inspected).
3. `9:ADD_DATABASE_PROTOCOL:exile:SQL_CUSTOM:SQL:exile.ini`, selecting database alias `exile`, SQL_CUSTOM protocol `SQL`, and the active INI basename `exile.ini`.
4. `9:LOCAL_TIME`, registers `LOG:TRADING`, `LOG:DEATH`, and `LOG:TERRITORY`, queries `9:OUTPUTSIZE`, then sends `9:LOCK` and marks the server connected.

Normal query helpers send `0:SQL:<payload>` for result-bearing calls and `1:SQL:<payload>` for fire-and-forget. The SQL payload starts with a named INI section followed by colon-separated arguments. For example, world vehicle loading calls `loadVehiclePage:<offset>:<pageSize>` through `ExileServer_system_database_query_selectFull`, and vehicle loading calls `loadVehicleSafe:<vehicleID>` through `...query_selectSingle`. Player updates build the `updatePlayer` named-query payload and send it through the fire-and-forget helper. These examples are in `ExileServer_world_loadAllDatabaseVehicles.sqf`, `ExileServer_object_vehicle_database_load.sqf`, and `ExileServer_object_player_database_update.sqf` in the extraction above.

The select helpers parse extDB3's array result. Status `0` is treated as a database error; status `2` invokes `ExileServer_system_database_handleBig`, which repeatedly requests `5:<key>` chunks until empty and compiles the assembled SQF value. `selectFull` returns the row set, `selectSingle` returns the first row, and `selectSingleField` returns the first cell. `query_insertSingle` returns the first response cell; fire-and-forget returns `true` after dispatch. The player update source explicitly ties payload field order and count to `exile.ini` `[updatePlayer] SQL1_INPUTS`.

## Active SQL_CUSTOM inventory

The active SQL_CUSTOM file is 81,879 bytes and has 301 bracket sections including `[Default]`. Its protocol-level `[Default]` settings are:

| Setting | Active value | Meaning recorded in INI comments |
|---|---|---|
| `Version` | `1` | SQL_CUSTOM INI format version |
| `Strip Chars` | `"/\\|;{}<>\\'` (INI literal) | Characters handled by input filtering |
| `Strip Chars Mode` | `1` | Strip and log bad characters |
| `Input SQF Parser` | `false` | Use colon-delimited inputs rather than the experimental SQF-array input parser |

The active query sections use `SQL<n>_<m>` statement fragments, `SQL<n>_INPUTS` parameter mappings, and optional `OUTPUT` column conversions. The active file has 283 `SQL1_INPUTS`, 25 `SQL2_INPUTS`, 7 `SQL3_INPUTS`, and one each for `SQL4_INPUTS` through `SQL8_INPUTS`; 128 `OUTPUT` declarations and 16 `Return InsertID` declarations. The `Number Of Custom Inputs` and `$CUSTOM_` examples under territory creation are commented out; active calls use prepared parameters and have no raw `$CUSTOM_` substitutions. The query names cover account/player state, vehicles, constructions, containers, clans, territories, cleanup, money and locker operations, notifications, raid and capture state, loadouts, email, units/AI groups, ELO, terrain modifications, and server state. The specific player/vehicle/construction/container/clan/territory names include `loadPlayer`, `updatePlayer`, `loadVehiclePage`, `loadVehicleSafe`, `loadConstruction`, `loadContainer`, `getClanInfo`, and `loadTerritory`.

An independent scan counted 1,002 active key/value entries: 534 SQL fragments, 320 input mappings, 128 output mappings, and 16 insert-ID declarations. The remaining active keys are `Version`, `Strip Chars`, `Strip Chars Mode`, and `Input SQF Parser`; these are accepted by the parser. No active `$CUSTOM_` substitution or `Number Of Custom Inputs` setting exists. The opt-in `consumer_configuration_if_supplied` test loaded the complete active INI and checked representative multi-statement and zero-input queries.

## Replay verification

The latest Windows and Linux test builds loaded the active `exile.ini` on MySQL 9.7.2 and replayed a 15-call fixture. It covers the source-derived bootstrap, vehicle page/safe loading, the RPT-observed `loadUnitsPage`, `readUnit`, and `loadTerritory` calls, and both fire-and-forget and synchronous `updatePlayer`. The vehicle queries returned the seeded 28-column row. The update used 42 synthetic values in the active SQF payload order; the isolated database row was checked afterward. All response strings matched the extDB3 1.031 reference replay on an isolated MySQL 8.0.46 instance. That reference-only INI uses bounded casts for long-text columns and shortened vehicle projections over the same synthetic rows because the legacy binary rejects `LONG_BLOB` results; the active INI itself was unchanged for replacement runs.

Read-only review of remote October 10 RPTs found eight warning-context signatures: five type-0 `loadUnitsPage` calls with two arguments, two type-0 `readUnit` calls with one argument, and one type-0 `loadTerritory` call with one argument. The argument counts match each active `SQL1_INPUTS` declaration. These RPT entries omit argument values; no literal `0:SQL:`, `1:SQL:`, or `2:SQL:` payload was available. The extDB3 log search found no raw `Input from Server` records. Three recent logs contain `2:SQL:slow` and `2:SQL:large` from the SQL_CUSTOM probe fixture, not from Exile. The replay therefore verifies production-observed call shapes with synthetic values, not exact captured payloads. No production database export was taken.

Other SQL_CUSTOM files present in the same directory are `avs.ini` (3 sections including `[Default]`, containing vehicle-ammo get/set queries), `exile.ini.extdb3default` (86 sections including `[Default]`, stock-like reference), and `exile.byner.extdb3transformed.ini.off` (125 sections; disabled by `.off`). Only `exile.ini` is named by the active bootstrap call observed above.

## Reference binaries and versions

The available native Windows x64 reference reports file and product version `1.0.3.1`. The available Linux `.so` references are 32-bit i386 ELF shared objects, not x86-64; the Windows x64 DLL is the reference used for differential replay.

## MySQL version evidence

The isolated harness reports MySQL 9.7.2; its test account uses `caching_sha2_password` with SSL required. No production database credentials or contents were queried or exported. The installed extDB3 1.031 reference could not connect to this MySQL 9 service, so direct reference parity on that server version remains unavailable.
