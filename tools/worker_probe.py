import argparse
import concurrent.futures
import json
import time

from abi_probe import Extension


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("library")
    parser.add_argument("--section", default="exile")
    parser.add_argument("--protocol", default="WORKER_PROBE")
    parser.add_argument("--workers", type=int, default=24)
    parser.add_argument("--sleep-seconds", type=float, default=2.0)
    args = parser.parse_args()
    if args.workers <= 16 or args.sleep_seconds <= 0:
        parser.error("workers must exceed 16 and sleep-seconds must be positive")

    extension = Extension(args.library)
    database = extension.invoke(f"9:ADD_DATABASE:{args.section}")
    if database != "[1]":
        raise SystemExit(f"database registration failed: {database}")
    protocol = extension.invoke(
        f"9:ADD_DATABASE_PROTOCOL:{args.section}:SQL_CUSTOM:{args.protocol}:worker-probe.ini"
    )
    if protocol != "[1]":
        raise SystemExit(f"protocol registration failed: {protocol}")

    call = f"0:{args.protocol}:hold:{args.sleep_seconds}"
    started = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as executor:
        responses = list(executor.map(lambda _: extension.invoke(call), range(args.workers)))
    elapsed = time.perf_counter() - started
    successful = sum(response.startswith("[1") for response in responses)
    threshold = args.sleep_seconds * 1.75
    print(
        json.dumps(
            {
                "workers": args.workers,
                "sleepSeconds": args.sleep_seconds,
                "successes": successful,
                "elapsedSeconds": round(elapsed, 3),
                "oneWaveThresholdSeconds": round(threshold, 3),
            },
            indent=2,
        )
    )
    return 0 if successful == args.workers and elapsed < threshold else 1


if __name__ == "__main__":
    raise SystemExit(main())
