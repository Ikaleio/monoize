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
            ["first", "-Wl,-Bstatic", "-lstdc++", "last"],
        )

    def test_requires_the_real_linker(self) -> None:
        environment = os.environ.copy()
        environment.pop("MONOIZE_MUSL_LINKER", None)
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
