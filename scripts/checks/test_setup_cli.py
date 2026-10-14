# SPDX-License-Identifier: MPL-2.0

"""Setup must validate the entire command before any optional action runs."""

import os
import shutil
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class SetupArguments(unittest.TestCase):
    def check_rejection(self, command):
        result = subprocess.run(
            command, cwd=ROOT, capture_output=True, text=True, timeout=15
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 2, output)
        self.assertIn("unknown setup option", output)
        # Setup reports this before any installation or machine configuration.
        self.assertNotIn("Koru setup check", output)
        self.assertNotIn("[third-party]", output)
        self.assertNotIn("[skills]", output)

    @unittest.skipUnless(os.name == "nt", "Windows launcher")
    def test_windows_rejects_unknown_tail_before_optional_actions(self):
        for options in [
            ["--invalid"],
            ["--agent-skills", "--defender-exclusions", "--invalid"],
            ["--check", "--agent-skills", "--defender-exclusions", "--invalid"],
        ]:
            with self.subTest(options=options):
                self.check_rejection([
                    "cmd.exe", "/d", "/c", str(ROOT / "koru.bat"), "setup", *options
                ])

    @unittest.skipUnless(os.name != "nt" and shutil.which("bash"), "Unix launcher")
    def test_unix_rejects_unknown_tail_before_optional_actions(self):
        for options in [
            ["--invalid"],
            ["--agent-skills", "--invalid"],
            ["--check", "--agent-skills", "--invalid"],
        ]:
            with self.subTest(options=options):
                self.check_rejection(["bash", str(ROOT / "koru.sh"), "setup", *options])


if __name__ == "__main__":
    unittest.main()
