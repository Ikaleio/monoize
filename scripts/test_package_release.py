from __future__ import annotations

from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

from scripts import package_release


PROJECT_ROOT = Path(__file__).resolve().parents[1]
TEST_ROOT = PROJECT_ROOT / "target" / "package-release-tests"


class PackageReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        TEST_ROOT.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=TEST_ROOT)
        self.root = Path(self.temporary.name)
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "monoize"\nversion = "1.0.0"\n', encoding="utf-8"
        )
        for name in package_release.DOCUMENTS:
            (self.root / name).write_text(f"{name}\n", encoding="utf-8")
        self.output = self.root / "dist"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_binary(self, target: str) -> Path:
        executable = package_release.TARGETS[target][1]
        path = self.root / "target" / target / "release" / executable
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(f"binary:{target}\n".encode())
        return path

    def test_tar_package_is_deterministic_and_has_required_modes(self) -> None:
        target = "x86_64-unknown-linux-musl"
        self.write_binary(target)
        archive, checksum = package_release.package_release(self.root, "v1.0.0", target, self.output)
        first_archive = archive.read_bytes()
        first_checksum = checksum.read_bytes()

        package_release.package_release(self.root, "v1.0.0", target, self.output)
        self.assertEqual(archive.read_bytes(), first_archive)
        self.assertEqual(checksum.read_bytes(), first_checksum)

        root_name = f"monoize-v1.0.0-{target}"
        with tarfile.open(archive, "r:gz") as bundle:
            names = bundle.getnames()
            self.assertEqual(
                names,
                [
                    root_name,
                    f"{root_name}/monoize",
                    f"{root_name}/LICENSE",
                    f"{root_name}/README.md",
                    f"{root_name}/README.zh-CN.md",
                ],
            )
            self.assertEqual(bundle.getmember(f"{root_name}/monoize").mode, 0o755)
            self.assertEqual(bundle.getmember(f"{root_name}/README.md").mode, 0o644)

    def test_windows_package_uses_zip_and_exe(self) -> None:
        target = "aarch64-pc-windows-msvc"
        self.write_binary(target)
        archive, _ = package_release.package_release(self.root, "v1.0.0", target, self.output)
        root_name = f"monoize-v1.0.0-{target}"
        self.assertEqual(archive.suffix, ".zip")
        with zipfile.ZipFile(archive) as bundle:
            self.assertEqual(
                bundle.namelist(),
                [
                    f"{root_name}/",
                    f"{root_name}/monoize.exe",
                    f"{root_name}/LICENSE",
                    f"{root_name}/README.md",
                    f"{root_name}/README.zh-CN.md",
                ],
            )

    def test_tag_must_match_cargo_version(self) -> None:
        target = "x86_64-unknown-linux-musl"
        self.write_binary(target)
        with self.assertRaises(package_release.ReleasePackagingError):
            package_release.package_release(self.root, "v1.0.1", target, self.output)

    def test_verify_accepts_supported_subset_and_valid_checksums(self) -> None:
        target = "x86_64-unknown-linux-musl"
        self.write_binary(target)
        package_release.package_release(self.root, "v1.0.0", target, self.output)

        entries = package_release.verify_release_directory(self.root, "v1.0.0", self.output)
        self.assertEqual(len(entries), 2)

    def test_verify_rejects_orphaned_unknown_and_empty_sets(self) -> None:
        self.output.mkdir(parents=True)
        with self.assertRaises(package_release.ReleasePackagingError):
            package_release.verify_release_directory(self.root, "v1.0.0", self.output)

        target = "x86_64-unknown-linux-musl"
        self.write_binary(target)
        archive, checksum = package_release.package_release(
            self.root, "v1.0.0", target, self.output
        )
        checksum.unlink()
        with self.assertRaises(package_release.ReleasePackagingError):
            package_release.verify_release_directory(self.root, "v1.0.0", self.output)

        archive.unlink()
        (self.output / "unknown.txt").write_text("unknown\n", encoding="utf-8")
        with self.assertRaises(package_release.ReleasePackagingError):
            package_release.verify_release_directory(self.root, "v1.0.0", self.output)

    def test_verify_complete_set_detects_corrupt_archive(self) -> None:
        for target in package_release.TARGETS:
            self.write_binary(target)
            package_release.package_release(self.root, "v1.0.0", target, self.output)

        entries = package_release.verify_release_directory(self.root, "v1.0.0", self.output)
        self.assertEqual(len(entries), 12)

        archive = self.output / package_release.archive_name(
            "v1.0.0", "x86_64-unknown-linux-musl"
        )
        archive.write_bytes(archive.read_bytes() + b"corrupt")
        with self.assertRaises(package_release.ReleasePackagingError):
            package_release.verify_release_directory(self.root, "v1.0.0", self.output)


if __name__ == "__main__":
    unittest.main()
