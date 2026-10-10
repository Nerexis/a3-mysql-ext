# Clean-room extDB3 work

- [x] Read the Principles section of the poteto-mode skill.
- [x] Phase A: Frame
- [x] Phase B: Design the workflow
- [x] Phase C: Run the loop
- [x] Inspect public contracts and actual Exile consumers without reading extDB3 implementation.
- [x] Compare two design packages and record independent cross-judgment.
- [x] Build ABI replay harness and baseline reference probes.
- [x] Implement request parsing, bounded jobs and result lifecycle.
- [x] Implement verified TLS pools and the server's SQL_CUSTOM option subset.
- [x] Build Windows MSVC and Ubuntu 24.04 Linux artifacts with Rust 1.99.0.
- [x] Verify MySQL 9.7.2 `caching_sha2_password` authentication over verified TLS on both targets; reject an unrelated CA.
- [x] Replay 15 Exile call shapes on both targets using the active SQL_CUSTOM INI and synthetic arguments, including the three query signatures found in production RPTs.
- [x] Read-only inspect the active Linux production SQL_CUSTOM files and RPT references. Confirm the production `exile.ini` and normalized `avs.ini` match the staged files, then replay the actual `getVehicleAmmo` and `setVehicleAmmo` call signatures against the isolated MySQL 9.7.2 clone on both targets.
- [x] Compare 23 SQL_CUSTOM probe calls with extDB3 1.031 on isolated MySQL 8.0.46, including errors, date/float formatting, async polling, and sync/async multipart fragments.
- [x] Review the latest published extDB3 release, v1.033.1: report protocol version 1.033, provide both Arma ABI interfaces on each target, and accept up to Arma's documented 2,048 extension arguments. Remove fixed active-job caps on both targets and retain bounded request queues plus per-database connection limits. Verified 32 concurrent Linux SQL_CUSTOM requests complete successfully.
- [ ] Replay exact Exile runtime argument values and run extDB3 1.031 directly against MySQL 9.7.2. The INI call signatures and RPT reference counts are verified, but exact runtime argument values were not available; the AVS replay uses a synthetic nonmatching ID. The reference binary cannot connect to MySQL 9.7.2.
- [x] Phase D: Keep the audit trail
- [x] Phase E: Verify and hand back, with the remaining acceptance gaps reported.

Architect phases: Ground, Sketch, Agree, Implement, Scrap.
Agree has no checkpoint because the user authorized implementation.
Arena phases: Frame, Fan out, Cross-judge, Pick, Graft, Verify.

Definition of done: both 64-bit artifacts build with the pinned toolchain and lockfile; ABI probes pass; RPT-observed Exile call shapes and SQL_CUSTOM parity fixtures pass; intended MySQL version passes caching_sha2_password authentication with verified TLS. Exact production argument replay and direct extDB3 comparison on MySQL 9.7.2 remain unavailable as noted above.

Scope: one new crate, three exports, command state machine, database/config layer, SQL_CUSTOM compiler, dynamic-library replay and differential tools, two build targets.
Rigor: high because persistence errors can corrupt game state. Only isolated test databases will receive replay writes.
Throughput checkpoint: after the first ABI build, measure remaining work by supported SQL_CUSTOM options and runnable acceptance fixtures. Missing server inputs remain explicit blockers.
