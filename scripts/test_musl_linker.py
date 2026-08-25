from __future__ import annotations

import os
from pathlib import Path
import subprocess
import unittest


PROJECT_ROOT = Path(__file__).resolve().parents[1]
LINKER_WRAPPER = PROJECT_ROOT / "scripts" / "musl-linker.sh"


class MuslLinkerTests(unittest.TestCase):
    def test_rewrites_dynamic_mode_and_preserves_other_arguments(self) -> None:
        environment = os.environ.copy()
        environment["MONOIZE_MUSL_LINKER"] = "/usr/bin/printf"
        environment["MONOIZE_MUSL_STATIC_LIBGCC"] = "false"
        result = subprocess.run(
            [
                LINKER_WRAPPER,
                "%s\n",
                "first",
                "-Wl,-Bdynamic",
                "-lstdc++",
                "last",
            ],
            cwd=PROJECT_ROOT,
            env=environment,
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(
            result.stdout.splitlines(),
            [
                "first",
                "-Wl,-Bstatic",
                "-Wl,--start-group",
                "-lstdc++",
                "-lc",
                "-Wl,--end-group",
                "last",
            ],
        )

    def test_adds_static_libgcc_for_aarch64_runtime_helpers(self) -> None:
        environment = os.environ.copy()
        environment["MONOIZE_MUSL_LINKER"] = "/usr/bin/printf"
        environment["MONOIZE_MUSL_STATIC_LIBGCC"] = "true"
        result = subprocess.run(
            [LINKER_WRAPPER, "%s\n", "-Wl,-Bdynamic", "-lstdc++"],
            cwd=PROJECT_ROOT,
            env=environment,
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(
            result.stdout.splitlines(),
            [
                "-Wl,-Bstatic",
                "-Wl,--start-group",
                "-lstdc++",
                "-lc",
                "-lgcc",
                "-Wl,--end-group",
            ],
        )

    def test_rejects_an_invalid_static_libgcc_switch(self) -> None:
        environment = os.environ.copy()
        environment["MONOIZE_MUSL_LINKER"] = "/usr/bin/printf"
        environment["MONOIZE_MUSL_STATIC_LIBGCC"] = "sometimes"
        result = subprocess.run(
            [LINKER_WRAPPER, "unused"],
            cwd=PROJECT_ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 64)
        self.assertIn("must equal true or false", result.stderr)

    def test_requires_the_real_linker(self) -> None:
        environment = os.environ.copy()
        environment.pop("MONOIZE_MUSL_LINKER", None)
        environment.pop("MONOIZE_MUSL_STATIC_LIBGCC", None)
        result = subprocess.run(
            [LINKER_WRAPPER, "unused"],
            cwd=PROJECT_ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MONOIZE_MUSL_LINKER must name", result.stderr)


if __name__ == "__main__":
    unittest.main()
