# Performance review — updated 2026-10-11

## Verdict

The synthetic benchmarks show the SQL_CUSTOM binder scaling near-linearly with input size after replacing per-character strip-set searches. At a fixed 4 KiB input, the 64-character strip-set case fell from 23.81 µs before the change to 9.3–9.5 µs in the optimized runs, about 60% lower. These are in-process measurements; they do not predict database or server startup latency.

The latest snapshot-restored comparison used the v0.1.0 Rust release DLL with default connection reset. Native extDB3 1.031 measured a 9.626 s median `WorldInitialize`; Rust measured 9.723 s. Rust was 0.097 s slower by the unpaired medians and 0.115 s slower by the paired median. The exact paired signed-rank test did not distinguish the backends (p = 0.3125). Rust remains 0.723 s above the under-9-second target.

The previous controlled Rust series measured 13.547 s median. The current release median is 28.2% lower after fixing idle pool retention, with the same Exile call and success counts. This is an improvement in the tested Rust configuration, not evidence that Rust is faster than native.

An earlier follow-up RPT attributes 8.599 s of a 9.659 s `WorldInitialize` to vehicle loading. That run points to mission object creation as the remaining startup limiter, rather than SQL driver time.

## Latest release comparison

The 2026-10-11 comparison used the v0.1.0 release DLL on the local Windows Arma server and MySQL 8.4.9 performance database. The tested Windows DLL SHA-256 was `b73079f9cf44ba5b893a2559584346d82bcb0497bb89578ce88ed1353a51c314`, matching the release package manifest. The harness restored the same SQL snapshot before each start and after the series. It alternated native extDB3 1.031 and Rust five times each. Native used automatic worker selection (`Thread = 0`); Rust used a 16-connection pool with default connection reset. RPT load lines confirmed the expected DLL for every run.

| Measurement | Native extDB3 1.031 | Rust-compatible 1.033 |
| --- | ---: | ---: |
| `WorldInitialize`, median (range) | 9.626 s (9.608–9.820) | 9.723 s (9.715–10.323) |
| Exile database call lines per run | 1,017 | 1,017 |
| Successful raw-result lines per run | 581 | 581 |
| Empty raw-result lines per run | 0 | 0 |
| Database or protocol error lines per run | 0 | 0 |

The paired Rust-minus-native differences were +0.206, -0.101, -0.003, +0.115, and +0.697 seconds. Their median was +0.115 seconds. The exact two-sided signed-rank test returned p = 0.3125, so these five pairs do not distinguish the backends. Rust was 0.723 seconds above the under-9-second target by its median.

Each run completed 1,017 SQL calls and 581 successful raw results, with no empty results or database/protocol errors. The harness restored the same SQL snapshot before each start and after the series. Each RPT contained the same CBA `HitEngine` expression and compile messages and RHS hitpoint-section compile messages. Those mission/mod errors appeared in every run and did not vary by backend. The query and result counts establish stable work volume, not byte-for-byte response parity. The summary JSON remains in the local ignored `.audit` directory; the ten startup RPTs remain in the local a3_2 server profile directory.

### Pool idle-retention diagnosis

The pool uses a zero minimum. With `mysql_async`'s default zero inactive-connection TTL, returned connections were discarded instead of being available for reuse. The configuration now retains idle connections for up to 60 seconds before the pool shrinks back to zero. A regression test checks that the configured TTL is nonzero.

One instrumented profile before the fix recorded 1,026 requests (942 synchronous, zero errors): synchronous pool acquisition summed to 3,490.57 ms, with 1,756 µs p50 and 12,049 µs p95. One profile after the fix recorded 1,023 requests (940 synchronous, zero errors): the sum was 3.29 ms, with 1 µs p50 and 3 µs p95. These profiles have slightly different request counts; their per-request component times overlap and the profiler adds overhead. Use them to locate the pool issue, not as end-to-end timing. The uninstrumented, snapshot-restored series above is the startup comparison.

## Synthetic measurements

The suite in `synthetic.rs` uses seven timed batches per point after a warmup and interleaves input sizes. Its log-log exponent, alpha, describes observed scaling over the tested range; it does not prove asymptotic complexity.

| Path | Input range | Expected work | Measured alpha | Largest-point observation |
| --- | --- | --- | ---: | --- |
| SQL_CUSTOM bind, input bytes | 64 B–256 KiB | O(B + M) expected | 0.81 overall; 1.05 at 16–256 KiB | 256 KiB: about 0.63 ms |
| SQL_CUSTOM bind, mapped arguments | 1–256 | O(B + M) expected | 0.90 | 256 fields: about 38.4 µs |
| SQL_CUSTOM parse, query sections | 1–1,024 | O(L + K log K) | 1.00 | 1,024 sections: about 1.25 ms |
| Result formatting, rows | 1–4,096, five columns | O(R·C + T) | 0.95 | 4,096 rows: about 2.16 ms |
| Multipart copy | 4 KiB–1 MiB, 4 KiB fragments | O(T) | 1.05 | 1 MiB: about 29–31 µs |
| SQF literal validation | 16 B–64 KiB | O(T) | 0.97 | 64 KiB: about 42 µs |
| SQF quoting | 16 B–64 KiB | O(T) | about 0.83 overall | 64 KiB: 203–262 µs across optimized runs |

Before the change, stripping checked membership in the configured character set for each input character, giving O(B·S) worst-case scan work for input length B and strip-set size S, followed by extra character-counting passes. The parser now prepares ASCII membership as a bitset and non-ASCII membership as a hash set; binding counts removed characters during the filtering pass. This gives O(B + M + S) expected work including one-time setup. For a 4 KiB input, measured time across strip sets of 1–64 characters stayed near 9.3–9.5 µs after the change; the observed slope against strip-set size fell from 0.156 to about 0.003.

Formatting 4,096 synthetic rows changed from a 3.09 ms pre-change median to about 2.16 ms in two optimized runs, roughly 30% lower. The formatter now borrows UTF-8 text from MySQL byte values and builds the response header and suffix in the row buffer. Cross-build absolute comparisons are indicative because the original baseline predates the interleaved measurement order. The unchanged SQF validator and the large-input quoting timings also varied across batches, so the within-series scaling shapes are stronger evidence than isolated cross-build timing changes.

The [baseline CSV](results/synthetic-before.csv) and two [interleaved optimized runs](results/synthetic-final-interleaved.csv) and [repeat](results/synthetic-final-interleaved-repeat.csv) preserve the measured data for the principal comparisons.

## Earlier controlled snapshot-restored comparison with default reset

An earlier default-reset build was measured in five alternating native/Rust starts on 2026-10-11. The performance database was restored from the same SQL snapshot before each start and restored again after the series. Rust loaded the workspace release DLL in every Rust start; native runs loaded the installed extDB3 1.031 DLL. The latest-release comparison above supersedes these timings.

| Measurement | Native extDB3 1.031 | Rust-compatible 1.033 |
| --- | ---: | ---: |
| `WorldInitialize`, median (range) | 9.666 s (9.625–9.986) | 9.835 s (9.779–9.937) |
| Exile database call lines per run | 1,017 | 1,017 |
| Successful raw-result lines per run | 581 | 581 |
| Empty raw-result lines per run | 0 | 0 |
| Database or protocol error lines per run | 0 | 0 |

Both backends used the same Windows Arma server and local MySQL 8.4.9 performance database. Native extDB3 used automatic worker selection (`Thread = 0`); Rust used a 16-connection pool. Rust used TLS with certificate and hostname verification skipped for the local loopback IP; transport remained TLS-required. These settings are not equally tuned. The user's interactive Arma client and Battleye were running throughout the series, while the dedicated server and UDP 2302 listener were absent between runs. The identical work counts and successful snapshot restores make this a controlled workload comparison, but not a driver-level profile or a claim that all responses were byte-for-byte identical. The overlapping ranges and p = 0.44 paired signed-rank result leave the small observed median gap inconclusive.

The user-defined target remains Rust below 9 seconds and faster than native. This tested build does not meet it.

## Controlled snapshot-restored comparison with reset disabled

A follow-up series used five alternating native/Rust pairs on 2026-10-11, restoring the same database snapshot before every start and after the series. The Rust test DLL disabled connection reset in code; a 60-second idle-connection TTL and 32-entry prepared-statement cache were used. Each backend completed all five starts.

| Measurement | Native extDB3 1.031 | Rust-compatible 1.033, reset off |
| --- | ---: | ---: |
| `WorldInitialize`, median (range) | 9.656 s (9.597–9.727) | 9.614 s (9.592–9.659) |
| Exile database call lines per run | 1,017 | 1,017 |
| Successful raw-result lines per run | 581 | 581 |
| Empty raw-result lines per run | 0 | 0 |
| Database or protocol error lines per run | 0 | 0 |

The paired Rust-minus-native differences were -0.116, +0.017, -0.064, +0.025, and -0.068 seconds (median -0.064 seconds; exact two-sided signed-rank p = 0.3125). This is a small and inconclusive difference; it does not establish that Rust is faster. Rust remains 0.614 seconds above the under-9-second target. Every run used the same Windows server and local MySQL 8.4.9 database, loaded the intended backend DLL, and kept work counts stable. Native used `Thread = 0`; Rust used a 16-connection pool. The server was stopped between runs and the final snapshot restore succeeded.

One Rust RPT from this reset-off series recorded 8.626 seconds for vehicle loading, 0.687 seconds for database units, 0.137 seconds for AI groups, and 0.143 seconds for containers, out of 9.653 seconds total. Vehicle loading accounted for about 89% of that run's world initialization, so SQL pool tuning alone cannot plausibly reach the target. This is one run's stage breakdown, not a repeated profile.

A separate one-run Exile smoke test with the configurable release DLL and `Reset Connection=false` took 9.653 seconds. It completed 1,017 SQL calls and 581 successful results, with no empty results or database/protocol errors, and the final database snapshot restore succeeded. The RPT confirms that Arma loaded the release DLL from the expected path. This validates the configuration option in the active server flow; it is one functional check, not another performance sample or a replacement for the five-pair comparison above.

The active Exile request trace contains 54 distinct SQL statement fragments across 1,023 requests. On the MySQL 9.7.2 synthetic replay, cache sizes 32 and 64 both prepared 54 statements after warmup and had similar median latency (551.9 µs and 553.3 µs); no cache-size change was justified. Reset-off substantially reduced repeated prepares for that replay, but these microbench timings do not predict startup speed. The source exposes the setting as `Reset Connection=false`; the default remains `true` because reset-off retains MySQL session variables and prepared statements across pooled requests.

## Earlier uncontrolled live Exile startup comparison

An earlier source build was measured in five alternating native/Rust starts on 2026-10-10 and 2026-10-11. The Rust extension loaded from the workspace release artifact in each Rust start, while native runs loaded the installed extDB3 1.031 DLL.

| Measurement | Native extDB3 1.031 | Rust-compatible 1.033 |
| --- | ---: | ---: |
| `WorldInitialize`, median (range) | 9.689 s (9.605–9.726) | 13.619 s (13.315–13.774) |
| Exile database call lines per run | 1,017 | 1,017–1,018 |
| Successful raw-result lines per run | 581 | 581 |
| Database, protocol, or TLS error lines per run | 0 | 0 |

The backends used the same Windows Arma server and local MySQL 8.4.9 database. Native extDB3 used automatic worker selection (`Thread = 0`); Rust used a 16-connection pool. Rust used TLS with certificate and hostname verification skipped for the local loopback IP; transport remained TLS-required. These are not equally tuned settings. Rust run 4 issued an additional `updateUnitRecruitmentTime`; runs 6 and 8 each had 11 fewer `updateAiGroup` and three fewer `updateVehicleLastGetIn` calls than run 2. Startup writes and evolving database/world state mean the workloads were not identical across repetitions.

### Historical pre-fast-path baseline

The earlier five-run comparison was not interleaved and measured source before the synchronous completion fast path. It remains useful as a historical reference, not as the latest-source result.

| Measurement | Native extDB3 1.031 | Rust-compatible 1.033 |
| --- | ---: | ---: |
| `WorldInitialize`, median (range) | 9.577 s (9.562–9.590) | 13.482 s (13.267–13.729) |
| Explicit `units` and `AI groups` database stages, median sum (range) | 0.858 s (0.846–0.904) | 1.972 s (1.907–2.060) |
| `postInit elapsed`, median (range) | 12.654 s (12.626–12.726) | 16.603 s (16.378–16.849) |
| Exile database call lines per run | 1,017 | 1,017 |
| Successful raw-result lines per run | 581 | 581 |
| Database, protocol, or TLS error lines per run | 0 | 0 |

Each backend completed five starts using the same Windows Arma server installation, Exile mission, local MySQL 8.4.9 service, and existing local database. The native runs were collected first, followed by five Rust runs. The database stages are about 28.5% of the 3.905 s median world-initialization gap; the remaining 2.791 s is not explained by the recorded stage timers. Matching call and success counts show comparable work volume, not byte-for-byte equality of every response.

The launch-to-ready stopwatch was not used as the comparison metric. RPT creation to extension load varied from about 25 to 102 seconds, with the direction of the outlier reversing between the two sets. Subtracting that delay yields only an approximate extension-to-ready estimate because RPT timestamps have one-second resolution.

The settings were not equal: native extDB3 used automatic worker selection (`Thread = 0`), while Rust used a 16-connection database pool. The Rust run also used the local loopback TLS setting with certificate and hostname verification skipped; transport remained TLS-required. The two backends differ in implementation and version. The startup runs used the existing local database and ordinary Exile writes; they were not read-only or run against an immutable snapshot. No query profiler or CPU profile was collected. The measurements establish that the tested Rust configuration was slower in this run, but they do not isolate the cause or establish that the Rust driver itself accounts for the gap.

The synthetic benchmarks use Rust 1.99.0 on an Intel Core i9-9900K. The live startup comparison used the same local Windows server and MySQL 8.4.9 for both backends. Startup captures, local database names, and RPTs remain local and are not included in this repository.
