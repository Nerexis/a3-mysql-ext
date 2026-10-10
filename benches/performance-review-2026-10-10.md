# Performance review — 2026-10-10

## Verdict

The synthetic benchmarks show the SQL_CUSTOM binder scaling near-linearly with input size after replacing per-character strip-set searches. At a fixed 4 KiB input, the 64-character strip-set case fell from 23.81 µs before the change to 9.3–9.5 µs in the optimized runs, about 60% lower. These are in-process measurements; they do not predict database or server startup latency.

The last live Exile comparison measured the Rust build before the synchronous completion fast path in `src/engine.rs`. The current source has passed release builds and ABI replays, but it has not been measured in a fresh startup run. Treat the live timings below as baseline data, not as results for this source revision.

In that comparison, the installed Rust-compatible 1.033 configuration had a median `WorldInitialize` time of 13.482 s (range 13.267–13.729 s), versus 9.577 s (9.562–9.590 s) for native extDB3 1.031. That is 3.905 s, or 40.8%, slower in that run. The result compares installed configurations, not equally tuned implementations. The cause is inconclusive: named database stages explain 1.114 s of the difference, and no profiler identified the remaining time.

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

## Live Exile startup comparison

| Measurement | Native extDB3 1.031 | Rust-compatible 1.033 |
| --- | ---: | ---: |
| `WorldInitialize`, median (range) | 9.577 s (9.562–9.590) | 13.482 s (13.267–13.729) |
| Explicit `units` and `AI groups` database stages, median sum (range) | 0.858 s (0.846–0.904) | 1.972 s (1.907–2.060) |
| `postInit elapsed`, median (range) | 12.654 s (12.626–12.726) | 16.603 s (16.378–16.849) |
| Exile database call lines per run | 1,017 | 1,017 |
| Successful raw-result lines per run | 581 | 581 |
| Database, protocol, or TLS error lines per run | 0 | 0 |

Each backend completed five starts using the same Windows Arma server installation, Exile mission, local MySQL 8.4.9 service, and existing local database. The native runs were collected first, followed by five Rust runs, rather than alternating the backends. The database stages are about 28.5% of the 3.905 s median world-initialization gap; the remaining 2.791 s is not explained by the recorded stage timers. Matching call and success counts show comparable work volume, not byte-for-byte equality of every response.

The launch-to-ready stopwatch was not used as the comparison metric. RPT creation to extension load varied from about 25 to 102 seconds, with the direction of the outlier reversing between the two sets. Subtracting that delay yields only an approximate extension-to-ready estimate because RPT timestamps have one-second resolution.

The settings were not equal: native extDB3 used automatic worker selection (`Thread = 0`), while Rust used a 16-connection database pool. The Rust run also used the local loopback TLS setting with certificate and hostname verification skipped; transport remained TLS-required. The two backends differ in implementation and version. The startup runs used the existing local database and ordinary Exile writes; they were not read-only or run against an immutable snapshot. No query profiler or CPU profile was collected. The measurements establish that the tested Rust configuration was slower in this run, but they do not isolate the cause or establish that the Rust driver itself accounts for the gap.

The synthetic benchmarks use Rust 1.99.0 on an Intel Core i9-9900K. The live startup comparison used the same local Windows server and MySQL 8.4.9 for both backends. Startup captures, local database names, and RPTs remain local and are not included in this repository.
