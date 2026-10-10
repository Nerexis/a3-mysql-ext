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
- [x] Verify MySQL 9.7.2 authentication, verified TLS, and source-derived Exile calls with synthetic payload values.
- [x] Compare the 16 shared calls with the saved extDB3 1.031 replay.
- [ ] Replay runtime-captured Exile calls and complete direct extDB3 differential testing on MySQL 9.7.2; no runtime capture was available and extDB3 1.031 returned `Database Config Error` on that server.
- [x] Phase D: Keep the audit trail
- [x] Phase E: Verify and hand back, with the remaining acceptance gaps reported.

Architect phases: Ground, Sketch, Agree, Implement, Scrap.
Agree has no checkpoint because the user authorized implementation.
Arena phases: Frame, Fan out, Cross-judge, Pick, Graft, Verify.

Definition of done: both 64-bit artifacts build with the pinned toolchain and lockfile; ABI probes and runtime-captured Exile calls pass; reference comparisons explain every mismatch; intended MySQL version passes caching_sha2_password authentication with verified TLS. Runtime captures and direct MySQL 9.7.2 reference comparisons remain outstanding as noted above.

Scope: one new crate, three exports, command state machine, database/config layer, SQL_CUSTOM compiler, dynamic-library replay and differential tools, two build targets.
Rigor: high because persistence errors can corrupt game state. Only isolated test databases will receive replay writes.
Throughput checkpoint: after the first ABI build, measure remaining work by supported SQL_CUSTOM options and runnable acceptance fixtures. Missing server inputs remain explicit blockers.
