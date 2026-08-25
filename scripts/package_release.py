#!/usr/bin/env python3
"""Create and verify native Monoize GitHub Release assets."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import os
from pathlib import Path
import sys
import tarfile
import tomllib
import zipfile


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DOCUMENTS = ("LICENSE", "README.md", "README.zh-CN.md")
TARGETS = {
    "x86_64-unknown-linux-musl": ("tar.gz", "monoize"),
    "aarch64-unknown-linux-musl": ("tar.gz", "monoize"),
    "x86_64-apple-darwin": ("tar.gz", "monoize"),
    "aarch64-apple-darwin": ("tar.gz", "monoize"),
    "x86_64-pc-windows-msvc": ("zip", "monoize.exe"),
    "aarch64-pc-windows-msvc": ("zip", "monoize.exe"),
}


class ReleasePackagingError(RuntimeError):
    """A release input or generated asset violates the release contract."""


def package_version(root: Path) -> str:
    """Return the Cargo package version from the selected project root."""

    manifest = root / "Cargo.toml"
    try:
        with manifest.open("rb") as handle:
            value = tomllib.load(handle)["package"]["version"]
    except (OSError, KeyError, tomllib.TOMLDecodeError) as error:
        raise ReleasePackagingError(f"cannot read package version from {manifest}: {error}") from error
    if not isinstance(value, str) or not value:
        raise ReleasePackagingError(f"invalid package version in {manifest}")
    return value


def validate_tag(root: Path, tag: str) -> str:
    """Require a release tag that exactly matches the Cargo package version."""

    version = package_version(root)
    expected = f"v{version}"
    if tag != expected:
        raise ReleasePackagingError(
            f"release tag {tag!r} does not match Cargo package version {version!r}; expected {expected!r}"
        )
    return version


def archive_name(tag: str, target: str) -> str:
    """Return the required archive basename for one supported target."""

    try:
        archive_format, _ = TARGETS[target]
    except KeyError as error:
        raise ReleasePackagingError(f"unsupported release target: {target}") from error
    suffix = ".tar.gz" if archive_format == "tar.gz" else ".zip"
    return f"monoize-{tag}-{target}{suffix}"


def allowed_asset_names(tag: str) -> set[str]:
    """Return every archive and checksum basename allowed for one release."""

    names: set[str] = set()
    for target in TARGETS:
        archive = archive_name(tag, target)
        names.add(archive)
        names.add(f"{archive}.sha256")
    return names


def sha256_file(path: Path) -> str:
    """Compute a lowercase SHA-256 digest without loading the asset at once."""

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _required_sources(root: Path, target: str) -> tuple[Path, tuple[Path, ...], str]:
    try:
        _, executable_name = TARGETS[target]
    except KeyError as error:
        raise ReleasePackagingError(f"unsupported release target: {target}") from error

    executable = root / "target" / target / "release" / executable_name
    documents = tuple(root / name for name in DOCUMENTS)
    for path in (executable, *documents):
        if not path.is_file():
            raise ReleasePackagingError(f"required release file does not exist: {path}")
    return executable, documents, executable_name


def _tar_info(name: str, mode: int, size: int = 0, is_directory: bool = False) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.type = tarfile.DIRTYPE if is_directory else tarfile.REGTYPE
    info.mode = mode
    info.size = size
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    return info


def _write_tar_gz(archive: Path, root_name: str, files: tuple[tuple[Path, str, int], ...]) -> None:
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as bundle:
                bundle.addfile(_tar_info(f"{root_name}/", 0o755, is_directory=True))
                for source, basename, mode in files:
                    data = source.read_bytes()
                    info = _tar_info(f"{root_name}/{basename}", mode, len(data))
                    bundle.addfile(info, io.BytesIO(data))


def _zip_info(name: str, mode: int, is_directory: bool = False) -> zipfile.ZipInfo:
    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    info.create_system = 3
    file_type = 0o040000 if is_directory else 0o100000
    info.external_attr = (file_type | mode) << 16
    info.compress_type = zipfile.ZIP_STORED if is_directory else zipfile.ZIP_DEFLATED
    return info


def _write_zip(archive: Path, root_name: str, files: tuple[tuple[Path, str, int], ...]) -> None:
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
        bundle.writestr(_zip_info(f"{root_name}/", 0o755, is_directory=True), b"")
        for source, basename, mode in files:
            bundle.writestr(_zip_info(f"{root_name}/{basename}", mode), source.read_bytes())


def package_release(root: Path, tag: str, target: str, output_dir: Path) -> tuple[Path, Path]:
    """Build one deterministic archive and its SHA-256 sidecar."""

    validate_tag(root, tag)
    executable, documents, executable_name = _required_sources(root, target)
    output_dir.mkdir(parents=True, exist_ok=True)

    basename = archive_name(tag, target)
    archive = output_dir / basename
    checksum = output_dir / f"{basename}.sha256"
    root_name = f"monoize-{tag}-{target}"
    files = (
        (executable, executable_name, 0o755),
        *((document, document.name, 0o644) for document in documents),
    )

    archive_format, _ = TARGETS[target]
    if archive_format == "tar.gz":
        _write_tar_gz(archive, root_name, files)
    else:
        _write_zip(archive, root_name, files)

    checksum.write_text(f"{sha256_file(archive)}  {archive.name}\n", encoding="utf-8", newline="\n")
    return archive, checksum


def verify_release_directory(root: Path, tag: str, directory: Path) -> tuple[Path, ...]:
    """Verify a non-empty subset of the supported target assets."""

    validate_tag(root, tag)
    if not directory.is_dir():
        raise ReleasePackagingError(f"release asset directory does not exist: {directory}")

    entries = tuple(sorted(directory.iterdir(), key=lambda path: path.name))
    if any(not entry.is_file() for entry in entries):
        raise ReleasePackagingError("release asset directory contains a non-file entry")

    actual = {entry.name for entry in entries}
    allowed = allowed_asset_names(tag)
    additional = sorted(actual - allowed)
    if additional:
        raise ReleasePackagingError(
            f"release asset set contains unknown files; additional={additional!r}"
        )

    present_targets: list[str] = []
    for target in TARGETS:
        name = archive_name(tag, target)
        archive = directory / name
        checksum = directory / f"{name}.sha256"
        if archive.is_file() != checksum.is_file():
            raise ReleasePackagingError(f"release asset pair is incomplete for target {target}")
        if not archive.is_file():
            continue
        present_targets.append(target)
        expected_line = f"{sha256_file(archive)}  {name}\n"
        try:
            actual_line = checksum.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            raise ReleasePackagingError(f"cannot read checksum file {checksum}: {error}") from error
        if actual_line != expected_line:
            raise ReleasePackagingError(f"checksum mismatch or malformed checksum file: {checksum}")

    if not present_targets:
        raise ReleasePackagingError("release asset directory contains no supported target")

    return entries


def _path(value: str) -> Path:
    return Path(value).resolve()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=_path, default=PROJECT_ROOT, help=argparse.SUPPRESS)
    subparsers = parser.add_subparsers(dest="command", required=True)

    package = subparsers.add_parser("package", help="create one target archive and checksum")
    package.add_argument("--tag", required=True)
    package.add_argument("--target", required=True, choices=tuple(TARGETS))
    package.add_argument("--output-dir", type=_path, required=True)

    verify = subparsers.add_parser("verify", help="verify the available release asset directory")
    verify.add_argument("--tag", required=True)
    verify.add_argument("--directory", type=_path, required=True)

    validate = subparsers.add_parser("validate", help="validate the tag against Cargo.toml")
    validate.add_argument("--tag", required=True)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.command == "package":
            assets = package_release(args.root, args.tag, args.target, args.output_dir)
        elif args.command == "verify":
            assets = verify_release_directory(args.root, args.tag, args.directory)
        else:
            print(validate_tag(args.root, args.tag))
            return 0
    except ReleasePackagingError as error:
        print(f"release packaging error: {error}", file=sys.stderr)
        return 1

    for asset in assets:
        print(os.fspath(asset))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
