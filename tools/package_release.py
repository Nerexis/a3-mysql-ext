import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from datetime import datetime, timezone
from pathlib import Path


TARGETS = {
    "x86_64-pc-windows-msvc": "extDB3_x64.dll",
    "x86_64-unknown-linux-gnu": "extDB3_x64.so",
}
RECEIPT_FIELDS = {
    "schemaVersion",
    "sourceCommit",
    "packageVersion",
    "targetTriple",
    "rustcVersion",
    "artifactName",
    "sha256",
}
RUSTC_PATTERN = re.compile(r"^rustc 1\.99\.0(?:\s|$)")
VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")
TAG_PATTERN = re.compile(r"^v(\d+\.\d+\.\d+)$")
COMMIT_PATTERN = re.compile(r"^[0-9a-f]{40,64}$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")


def run(command, cwd):
    print("+ " + " ".join(str(part) for part in command), flush=True)
    subprocess.run(command, cwd=cwd, check=True)


def output(command, cwd):
    return subprocess.run(
        command, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.strip()


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for block in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def source_commit(repo_root):
    status = output(
        ["git", "status", "--porcelain", "--untracked-files=normal"], repo_root
    )
    if status:
        raise RuntimeError("Release packaging requires a clean Git worktree.")
    commit = output(["git", "rev-parse", "HEAD"], repo_root)
    if not COMMIT_PATTERN.fullmatch(commit):
        raise RuntimeError("Git returned an invalid source commit.")
    return commit


def package_version(repo_root):
    metadata = json.loads(
        output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--no-deps"],
            repo_root,
        )
    )
    root_manifest = (repo_root / "Cargo.toml").resolve()
    version = next(
        (
            package["version"]
            for package in metadata["packages"]
            if Path(package["manifest_path"]).resolve() == root_manifest
        ),
        None,
    )
    if version is None:
        raise RuntimeError("Cargo metadata did not include the root package.")
    if not VERSION_PATTERN.fullmatch(version):
        raise RuntimeError("Cargo.toml package version must use MAJOR.MINOR.PATCH.")
    return version


def release_version(args, package_version):
    if args.tag:
        match = TAG_PATTERN.fullmatch(args.tag)
        if not match:
            raise RuntimeError("Release tag must use the form vMAJOR.MINOR.PATCH.")
        version = match.group(1)
        if version != package_version:
            raise RuntimeError(
                f"Release tag {args.tag} does not match Cargo.toml version {package_version}."
            )
        return version
    if args.version != package_version:
        raise RuntimeError(
            f"Requested release version {args.version} does not match Cargo.toml version {package_version}."
        )
    return args.version


def read_receipts(artifact_root, commit, version):
    expected_dirs = set(TARGETS)
    if not artifact_root.is_dir():
        raise RuntimeError(f"Native artifact directory is missing: {artifact_root}")
    entries = {entry.name for entry in artifact_root.iterdir()}
    if entries != expected_dirs:
        raise RuntimeError(
            "Native artifact directory must contain exactly one Windows and one Linux target."
        )

    receipts = {}
    artifacts = {}
    for target, artifact_name in TARGETS.items():
        target_dir = artifact_root / target
        if not target_dir.is_dir():
            raise RuntimeError(f"Native target directory is missing: {target}")
        children = {entry.name for entry in target_dir.iterdir()}
        if children != {"receipt.json", artifact_name}:
            raise RuntimeError(
                f"Native target {target} must contain only receipt.json and {artifact_name}."
            )

        receipt_path = target_dir / "receipt.json"
        artifact_path = target_dir / artifact_name
        if receipt_path.is_symlink() or artifact_path.is_symlink():
            raise RuntimeError(f"Native target {target} contains a symbolic link.")
        if not receipt_path.is_file() or not artifact_path.is_file():
            raise RuntimeError(f"Native target {target} is missing a regular file.")
        try:
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise RuntimeError(f"Cannot read receipt for {target}: {error}") from error
        if not isinstance(receipt, dict) or set(receipt) != RECEIPT_FIELDS:
            raise RuntimeError(f"Receipt fields do not match schema version 1 for {target}.")
        if type(receipt["schemaVersion"]) is not int or receipt["schemaVersion"] != 1:
            raise RuntimeError(f"Unsupported receipt schema for {target}.")
        if receipt["sourceCommit"] != commit:
            raise RuntimeError(f"Receipt for {target} names a stale source commit.")
        if receipt["packageVersion"] != version:
            raise RuntimeError(f"Receipt for {target} names a stale package version.")
        if receipt["targetTriple"] != target:
            raise RuntimeError(f"Receipt for {target} names the wrong target triple.")
        if receipt["artifactName"] != artifact_name:
            raise RuntimeError(f"Receipt for {target} names the wrong artifact file.")
        if not isinstance(receipt["rustcVersion"], str) or not RUSTC_PATTERN.match(
            receipt["rustcVersion"]
        ):
            raise RuntimeError(f"Receipt for {target} names the wrong Rust compiler.")
        if not isinstance(receipt["sha256"], str) or not SHA256_PATTERN.fullmatch(
            receipt["sha256"]
        ):
            raise RuntimeError(f"Receipt for {target} has an invalid SHA-256 value.")
        if sha256(artifact_path) != receipt["sha256"]:
            raise RuntimeError(f"SHA-256 mismatch for native target {target}.")
        receipts[target] = receipt
        artifacts[target] = artifact_path

    windows_target = "x86_64-pc-windows-msvc"
    linux_target = "x86_64-unknown-linux-gnu"
    if receipts[windows_target]["rustcVersion"] != receipts[linux_target]["rustcVersion"]:
        raise RuntimeError("Windows and Linux receipts use different Rust compiler builds.")
    return receipts, artifacts


def check_zip(path, expected_entries):
    with zipfile.ZipFile(path, "r") as archive:
        actual_entries = set(archive.namelist())
        missing = set(expected_entries) - actual_entries
        if missing:
            raise RuntimeError(
                f"Archive {path.name} is missing required entries: {', '.join(sorted(missing))}."
            )


def package(repo_root, artifact_root, output_dir, version, commit):
    receipts, artifacts = read_receipts(artifact_root, commit, version)
    package_name = f"exile-extdb3-v{version}-win64-linux-x64"
    runtime_name = f"{package_name}.zip"
    source_name = f"{package_name}-source.zip"
    output_dir.mkdir(parents=True, exist_ok=True)

    manifest = {
        "release": version,
        "license": "LGPL-3.0-only",
        "sourceCommit": commit,
        "protocolVersion": "1.033",
        "compatibilityTarget": "extDB3 v1.033.1",
        "differentialReference": "extDB3 v1.031",
        "rustc": receipts["x86_64-pc-windows-msvc"]["rustcVersion"],
        "linuxRustc": receipts["x86_64-unknown-linux-gnu"]["rustcVersion"],
        "builtAtUtc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "targets": list(TARGETS),
        "artifacts": [
            {
                "path": "windows-x64/extDB3_x64.dll",
                "sha256": receipts["x86_64-pc-windows-msvc"]["sha256"],
            },
            {
                "path": "linux-x64/extDB3_x64.so",
                "sha256": receipts["x86_64-unknown-linux-gnu"]["sha256"],
            },
        ],
    }
    runtime_entries = {
        "README.md": repo_root / "release" / "README.md",
        "LICENSE": repo_root / "LICENSE",
        "THIRD-PARTY-NOTICES.md": repo_root / "THIRD-PARTY-NOTICES.md",
        "windows-x64/extDB3_x64.dll": artifacts["x86_64-pc-windows-msvc"],
        "linux-x64/extDB3_x64.so": artifacts["x86_64-unknown-linux-gnu"],
    }
    required_runtime = set(runtime_entries) | {"manifest.json"}
    required_source = {
        f"{package_name}-source/README.md",
        f"{package_name}-source/LICENSE",
        f"{package_name}-source/THIRD-PARTY-NOTICES.md",
        f"{package_name}-source/tools/generate-third-party-notices.ps1",
        f"{package_name}-source/Cargo.toml",
        f"{package_name}-source/Cargo.lock",
        f"{package_name}-source/src/lib.rs",
    }

    with tempfile.TemporaryDirectory(prefix=".package-", dir=output_dir) as stage_text:
        stage = Path(stage_text)
        manifest_path = stage / "manifest.json"
        manifest_path.write_text(
            json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
        )
        runtime_path = stage / runtime_name
        with zipfile.ZipFile(
            runtime_path,
            "w",
            compression=zipfile.ZIP_DEFLATED,
            compresslevel=9,
        ) as archive:
            for archive_name, source_path in runtime_entries.items():
                archive.write(source_path, archive_name)
            archive.write(manifest_path, "manifest.json")
        check_zip(runtime_path, required_runtime)
        with zipfile.ZipFile(runtime_path, "r") as archive:
            packaged_manifest = json.loads(archive.read("manifest.json"))
            if packaged_manifest != manifest:
                raise RuntimeError("Packaged manifest does not match the validated inputs.")

        source_path = stage / source_name
        run(
            [
                "git",
                "archive",
                "--format=zip",
                f"--prefix={package_name}-source/",
                f"--output={source_path}",
                commit,
            ],
            repo_root,
        )
        check_zip(source_path, required_source)

        runtime_hash = sha256(runtime_path)
        source_hash = sha256(source_path)
        (stage / f"{runtime_name}.sha256").write_text(
            f"{runtime_hash} *{runtime_name}\n", encoding="ascii"
        )
        (stage / f"{source_name}.sha256").write_text(
            f"{source_hash} *{source_name}\n", encoding="ascii"
        )
        for name in (
            runtime_name,
            f"{runtime_name}.sha256",
            source_name,
            f"{source_name}.sha256",
        ):
            shutil.copy2(stage / name, output_dir / name)

    print(f"Release archive: {output_dir / runtime_name}")
    print(f"SHA-256 file: {output_dir / (runtime_name + '.sha256')}")
    print(f"Source archive: {output_dir / source_name}")
    print(f"Source SHA-256 file: {output_dir / (source_name + '.sha256')}")


def main():
    parser = argparse.ArgumentParser(
        description="Package validated native artifacts into matching runtime and source archives."
    )
    version_group = parser.add_mutually_exclusive_group(required=True)
    version_group.add_argument("--version")
    version_group.add_argument("--tag")
    parser.add_argument("--artifact-root", type=Path, default=Path("dist/native"))
    parser.add_argument("--output-dir", type=Path, default=Path("dist/release"))
    parser.add_argument("--source-commit")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    commit = source_commit(repo_root)
    if args.source_commit and args.source_commit != commit:
        raise RuntimeError("Requested source commit does not match the checked-out commit.")
    package_version_value = package_version(repo_root)
    version = release_version(args, package_version_value)
    artifact_root = args.artifact_root.resolve()
    output_dir = args.output_dir.resolve()
    package(repo_root, artifact_root, output_dir, version, commit)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"package_release.py: {error}", file=sys.stderr)
        sys.exit(1)
