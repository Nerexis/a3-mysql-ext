import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


TARGETS = {
    "x86_64-pc-windows-msvc": {
        "host": "Windows",
        "cargo_artifact": "extDB3_x64.dll",
        "artifact_name": "extDB3_x64.dll",
    },
    "x86_64-unknown-linux-gnu": {
        "host": "Linux",
        "cargo_artifact": "libextDB3_x64.so",
        "artifact_name": "extDB3_x64.so",
    },
}
TOOLCHAIN_PATTERN = re.compile(r"^rustc 1\.99\.0(?:\s|$)")
COMMIT_PATTERN = re.compile(r"^[0-9a-f]{40,64}$")


def run(command, cwd, env=None):
    print("+ " + " ".join(str(part) for part in command), flush=True)
    subprocess.run(command, cwd=cwd, env=env, check=True)


def output(command, cwd):
    return subprocess.run(
        command, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.strip()


def cargo_package_version(repo_root, cargo):
    metadata = json.loads(
        output(
            [cargo, "metadata", "--format-version", "1", "--locked", "--no-deps"],
            repo_root,
        )
    )
    root_manifest = (repo_root / "Cargo.toml").resolve()
    for package in metadata["packages"]:
        if Path(package["manifest_path"]).resolve() == root_manifest:
            return package["version"]
    raise RuntimeError("Cargo metadata did not include the root package.")


def git_source_commit(repo_root):
    status = output(
        ["git", "status", "--porcelain", "--untracked-files=normal"], repo_root
    )
    if status:
        raise RuntimeError("Native release checks require a clean Git worktree.")
    commit = output(["git", "rev-parse", "HEAD"], repo_root)
    if not COMMIT_PATTERN.fullmatch(commit):
        raise RuntimeError("Git returned an invalid source commit.")
    return commit


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for block in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(
        description="Run the release checks for one native target and write its artifact receipt."
    )
    parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--source-commit")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    target = TARGETS[args.target]
    if platform.system() != target["host"]:
        raise RuntimeError(
            f"Target {args.target} requires a {target['host']} native runner."
        )

    if args.source_commit:
        if not COMMIT_PATTERN.fullmatch(args.source_commit):
            raise RuntimeError("Requested source commit is invalid.")
        source_commit = args.source_commit
    else:
        source_commit = git_source_commit(repo_root)
    package_version = cargo_package_version(repo_root, args.cargo)
    rustc_version = output([args.rustc, "--version"], repo_root)
    if not TOOLCHAIN_PATTERN.match(rustc_version):
        raise RuntimeError(
            f"Release checks require Rust 1.99.0; found {rustc_version!r}."
        )

    cargo_env = os.environ.copy()
    cargo_commands = [
        [args.cargo, "fmt", "--all", "--", "--check"],
        [args.cargo, "test", "--locked", "--target", args.target],
        [
            args.cargo,
            "clippy",
            "--locked",
            "--all-targets",
            "--target",
            args.target,
            "--",
            "-D",
            "warnings",
        ],
        [args.cargo, "build", "--locked", "--release", "--target", args.target],
        [
            args.cargo,
            "bench",
            "--locked",
            "--bench",
            "synthetic",
            "--features",
            "synthetic-bench",
            "--no-run",
            "--target",
            args.target,
        ],
    ]
    for command in cargo_commands:
        run(command, repo_root, cargo_env)

    cargo_target_dir = Path(cargo_env.get("CARGO_TARGET_DIR", repo_root / "target"))
    if not cargo_target_dir.is_absolute():
        cargo_target_dir = repo_root / cargo_target_dir
    cargo_artifact = (
        cargo_target_dir / args.target / "release" / target["cargo_artifact"]
    )
    if not cargo_artifact.is_file():
        raise RuntimeError(f"Cargo release artifact is missing: {cargo_artifact}")

    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    artifact_path = output_dir / target["artifact_name"]
    receipt_path = output_dir / "receipt.json"
    shutil.copy2(cargo_artifact, artifact_path)
    with tempfile.TemporaryDirectory(prefix="extdb3-abi-") as temp_dir:
        run(
            [
                sys.executable,
                str(repo_root / "tools" / "abi_probe.py"),
                str(artifact_path),
                "--output",
                str(Path(temp_dir) / "abi.json"),
            ],
            repo_root,
            cargo_env,
        )

    receipt = {
        "schemaVersion": 1,
        "sourceCommit": source_commit,
        "packageVersion": package_version,
        "targetTriple": args.target,
        "rustcVersion": rustc_version,
        "artifactName": target["artifact_name"],
        "sha256": sha256(artifact_path),
    }
    receipt_path.write_text(
        json.dumps(receipt, indent=2) + "\n", encoding="utf-8"
    )
    print(f"Native artifact and receipt written to {output_dir}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"check_native.py: {error}", file=sys.stderr)
        sys.exit(1)
