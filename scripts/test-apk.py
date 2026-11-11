"""Exercise packaging decisions with fake tools; never sign or install an APK."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class Packaging(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="herdc-apk-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        app = self.root / "app"
        app.mkdir()
        shutil.copyfile(ROOT / "app/apk.sh", app / "apk.sh")
        shutil.copyfile(ROOT / "app/build-number", app / "build-number")
        (self.root / "env.sh").write_text("")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.called = self.root / "called"
        self.key = self.root / "release.jks"
        self.env = dict(os.environ, PATH=f"{self.bin}:/usr/bin:/bin",
                        HERDC_KEYSTORE=str(self.key), CALLED=str(self.called))
        self.env.pop("HERDC_BUILD", None)
        self.tool("nsh", 'printf "%s\\n" "$@" > "$CALLED"')
        self.tool("security", "exit 1")

    def tool(self, name, body):
        path = self.bin / name
        path.write_text(f"#!/bin/sh\n{body}\n")
        path.chmod(0o755)

    def run_apk(self, *args):
        return subprocess.run(["sh", str(self.root / "app/apk.sh"), *args],
                              env=self.env, capture_output=True, text=True)

    def test_release_never_falls_back_to_debug(self):
        self.assertNotEqual(self.run_apk().returncode, 0)
        self.assertFalse(self.called.exists())
        self.key.touch()
        self.assertNotEqual(self.run_apk().returncode, 0)
        self.assertFalse(self.called.exists())

    def test_explicit_debug_needs_no_release_key(self):
        self.assertEqual(self.run_apk("--debug").returncode, 0)
        self.assertNotIn("--keystore", self.called.read_text())

    def test_release_uses_requested_key(self):
        self.key.touch()
        self.tool("security", "echo synthetic-test-password")
        self.assertEqual(self.run_apk().returncode, 0)
        self.assertIn(str(self.key), self.called.read_text())

    def test_build_is_independent_of_git_and_above_recovered_apk(self):
        result = self.run_apk("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        default_build = int((ROOT / "app/build-number").read_text())
        self.assertGreaterEqual(default_build, 50)
        self.assertIn(f"Build {default_build} ", result.stdout)
        for build in ("49", "0", "050", "bad", "2100000001", "99999999999999999999"):
            self.env["HERDC_BUILD"] = build
            self.assertNotEqual(self.run_apk("--debug").returncode, 0, build)
        self.assertFalse(self.called.exists())
        self.env["HERDC_BUILD"] = str(default_build + 1)
        self.assertEqual(self.run_apk("--debug").returncode, 0)
        self.assertIn(str(default_build + 1), self.called.read_text().splitlines())


if __name__ == "__main__":
    unittest.main()
