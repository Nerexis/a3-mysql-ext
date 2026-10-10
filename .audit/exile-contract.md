# Exile / extDB3 consumer contract

Scope: read-only inspection of the active server addon extraction, its SQL_CUSTOM INIs, extDB3 reference binaries, and project documentation. No credentials, password files, DB dumps, or extDB3 connection configuration values were read. No extDB3 implementation source was inspected.

## Call flow

The active consumer extraction is `E:\Projekty\arma-3-cloud-server-instance\tmp\active_exile_pja314_20260825_extract\Exile_Server_Overrides`. Its database bootstrap, `ExileServer_system_database_connect.sqf`, calls the extension `extDB3` in this order:

1. `9:VERSION`, then rejects a numeric result below `1.027`.
2. `9:ADD_DATABASE:exile` (connection details are delegated to the extension's external configuration; values were not inspected).
3. `9:ADD_DATABASE_PROTOCOL:exile:SQL_CUSTOM:SQL:exile.ini`, selecting database alias `exile`, SQL_CUSTOM protocol `SQL`, and the active INI basename `exile.ini`.
4. `9:LOCAL_TIME`, registers `LOG:TRADING`, `LOG:DEATH`, and `LOG:TERRITORY`, queries `9:OUTPUTSIZE`, then sends `9:LOCK` and marks the server connected.

Normal query helpers send `0:SQL:<payload>` for result-bearing calls and `1:SQL:<payload>` for fire-and-forget. The SQL payload starts with a named INI section followed by colon-separated arguments. For example, world vehicle loading calls `loadVehiclePage:<offset>:<pageSize>` through `ExileServer_system_database_query_selectFull`, and vehicle loading calls `loadVehicleSafe:<vehicleID>` through `...query_selectSingle`. Player updates build the `updatePlayer` named-query payload and send it through the fire-and-forget helper. These examples are in `ExileServer_world_loadAllDatabaseVehicles.sqf`, `ExileServer_object_vehicle_database_load.sqf`, and `ExileServer_object_player_database_update.sqf` in the extraction above.

The select helpers parse extDB3's array result. Status `0` is treated as a database error; status `2` invokes `ExileServer_system_database_handleBig`, which repeatedly requests `5:<key>` chunks until empty and compiles the assembled SQF value. `selectFull` returns the row set, `selectSingle` returns the first row, and `selectSingleField` returns the first cell. `query_insertSingle` returns the first response cell; fire-and-forget returns `true` after dispatch. The player update source explicitly ties payload field order and count to `exile.ini` `[updatePlayer] SQL1_INPUTS`.

## Active SQL_CUSTOM inventory

The active file is `E:\Gry\A3Server\server\mods\@extdb3\sql_custom\exile.ini` (81,879 bytes; 301 bracket sections including `[Default]`). Its protocol-level `[Default]` settings are:

| Setting | Active value | Meaning recorded in INI comments |
|---|---|---|
| `Version` | `1` | SQL_CUSTOM INI format version |
| `Strip Chars` | `"/\\|;{}<>\\'` (INI literal) | Characters handled by input filtering |
| `Strip Chars Mode` | `1` | Strip and log bad characters |
| `Input SQF Parser` | `false` | Use colon-delimited inputs rather than the experimental SQF-array input parser |

The active query sections use `SQL<n>_<m>` statement fragments, `SQL<n>_INPUTS` parameter mappings, and optional `OUTPUT` column conversions. The active file has 283 `SQL1_INPUTS`, 25 `SQL2_INPUTS`, 7 `SQL3_INPUTS`, and one each for `SQL4_INPUTS` through `SQL8_INPUTS`; 128 `OUTPUT` declarations and 16 `Return InsertID` declarations. The `Number Of Custom Inputs` and `$CUSTOM_` examples under territory creation are commented out; active calls use prepared parameters and have no raw `$CUSTOM_` substitutions. The query names cover account/player state, vehicles, constructions, containers, clans, territories, cleanup, money and locker operations, notifications, raid and capture state, loadouts, email, units/AI groups, ELO, terrain modifications, and server state. The specific player/vehicle/construction/container/clan/territory names include `loadPlayer`, `updatePlayer`, `loadVehiclePage`, `loadVehicleSafe`, `loadConstruction`, `loadContainer`, `getClanInfo`, and `loadTerritory`.

## Replay verification

The test harness loaded the copied active `exile.ini` on MySQL 9.7.2 and replayed the source-derived bootstrap, `loadVehiclePage:0:10`, `loadVehicleSafe:1`, and both fire-and-forget and synchronous `updatePlayer` calls. The vehicle queries returned the seeded 28-column row. The update used 42 synthetic values in the order from the active SQF payload builder; the isolated database row was checked afterward. This verifies the consumer call shape and field mapping, not a runtime-captured production payload.

Other SQL_CUSTOM files present in the same directory are `avs.ini` (3 sections including `[Default]`, containing vehicle-ammo get/set queries), `exile.ini.extdb3default` (86 sections including `[Default]`, stock-like reference), and `exile.byner.extdb3transformed.ini.off` (125 sections; disabled by `.off`). Only `exile.ini` is named by the active bootstrap call observed above.

## Reference binaries and versions

`E:\Gry\A3Server\server\mods\@extdb3` contains `extDB3.dll` (1,288,968 bytes), `extDB3_x64.dll` (1,723,656 bytes), and `extDB3.so` (5,520,576 bytes). Windows DLL file and product version metadata report `1.0.3.1`; the Linux `.so` has no Windows version metadata. A second folder, `E:\Gry\A3Server\server\mods\@extdb3_a3_1`, also contains these three binaries; its Windows DLLs likewise report `1.0.3.1`.

## MySQL version evidence

No MySQL or MariaDB server version was found in the nonsensitive project documentation inspected (`E:\Projekty\arma-3-cloud-server-instance\docs\hetzner-standby-runbook.md` and its root `AGENTS.md`). The runbook confirms the server uses MySQL, but specifies no version. I did not inspect database credentials or query the live server.
