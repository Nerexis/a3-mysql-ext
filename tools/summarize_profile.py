import argparse
import csv
import math
from collections import defaultdict
from pathlib import Path


TIMING_COLUMNS = (
    "end_to_end_us",
    "mysql_async_us",
    "pool_acquire_us",
    "format_us",
    "job_queue_us",
)


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser(description="Summarize extDB3 request profile CSV.")
    parser.add_argument("profile", type=Path)
    parser.add_argument("--limit", type=int, default=30)
    args = parser.parse_args()
    if args.limit < 1:
        parser.error("--limit must be positive")

    groups = defaultdict(list)
    with args.profile.open(newline="", encoding="utf-8-sig") as source:
        for row in csv.DictReader(source):
            for column in TIMING_COLUMNS + ("rows", "response_bytes"):
                row[column] = int(row[column])
            groups[(row["kind"], row["query"])].append(row)

    if not groups:
        raise SystemExit("profile contains no request records")

    total = sum(map(len, groups.values()))
    errors = sum(
        row["outcome"] != "ok"
        for records in groups.values()
        for row in records
    )
    print(f"requests={total} errors={errors} query_groups={len(groups)}")
    print(
        "kind query calls errors p50_e2e_us p95_e2e_us "
        "p50_mysql_async_us p95_mysql_async_us p95_pool_us p95_format_us rows"
    )

    ordered_groups = sorted(
        groups.items(),
        key=lambda item: percentile(
            [row["end_to_end_us"] for row in item[1]], 0.50
        ),
        reverse=True,
    )
    for (kind, query), records in ordered_groups[: args.limit]:
        errors = sum(row["outcome"] != "ok" for row in records)
        metric = {
            column: [row[column] for row in records]
            for column in TIMING_COLUMNS
        }
        rows = sum(row["rows"] for row in records)
        print(
            f"{kind} {query} {len(records)} {errors} "
            f"{percentile(metric['end_to_end_us'], 0.50)} "
            f"{percentile(metric['end_to_end_us'], 0.95)} "
            f"{percentile(metric['mysql_async_us'], 0.50)} "
            f"{percentile(metric['mysql_async_us'], 0.95)} "
            f"{percentile(metric['pool_acquire_us'], 0.95)} "
            f"{percentile(metric['format_us'], 0.95)} {rows}"
        )


if __name__ == "__main__":
    main()
