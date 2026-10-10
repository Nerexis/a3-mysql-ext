# Synthetic performance suite

Run the release-mode suite from the repository root:

    cargo bench --bench synthetic --features synthetic-bench -- .audit/synthetic-benchmark.csv

It uses only in-process synthetic data. It does not connect to MySQL, start
Arma, or write to a configured database. The benchmark-only API is enabled by
synthetic-bench and is not present in ordinary builds.

The CSV includes the median, minimum, and maximum time per operation from seven
samples, the work size, estimated throughput, and an observed scaling exponent
for each series. Each series interleaves small and large inputs to reduce
time-order bias. The exponent is the least-squares slope of log(time) against
log(size): approximately 1 is consistent with linear scaling over the tested
range, while approximately 2 is consistent with quadratic scaling. This is an
empirical comparison, not a proof of asymptotic complexity; small workloads
include timer and call overhead.

Coverage:

- SQF literal validation and quoting over 16 B to 64 KiB strings.
- SQL_CUSTOM parse over 1 to 1,024 query sections.
- SQL_CUSTOM bind over 1 to 256 mapped arguments, 64 B to 256 KiB input, and
  strip-character sets of 1 to 64 characters.
- Production result-row formatting over 1 to 4,096 synthetic rows.
- Multipart result copying over 4 KiB to 1 MiB responses using 4 KiB fragments.

Compare before and after CSV files from the same machine, release toolchain,
power profile, and background load. The synthetic results isolate CPU and
allocation work; they do not model MySQL latency, worker contention, the Arma
scheduler, or full server startup.

The [performance review](performance-review-2026-10-10.md) records the current
synthetic measurements and the separate live Exile startup comparison. The
small CSV files in `results/` preserve the baseline and two repeated optimized
runs; the baseline predates the interleaved ordering adjustment.
