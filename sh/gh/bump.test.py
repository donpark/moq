"""Find hand-versioned packages with unreleased changes on a first-parent history."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("bump", Path(__file__).with_name("bump.py"))
bump = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bump)


def package_json(name: str, version: str, private: bool = False) -> str:
    manifest = {"name": name, "version": version, "scripts": {"release": "bun ../common/release.ts"}}
    if private:
        manifest["private"] = True
    return json.dumps(manifest, indent="\t") + "\n"


BUN_LOCK = """{
  "workspaces": {
    "js/a": {
      "name": "@moq/a",
      "version": "0.1.0",
    },
    "js/b": {
      "name": "@moq/b",
      "version": "0.2.0",
    },
  },
}
"""

PYPROJECT = """[build-system]
requires = ["hatchling"]

[project]
name = "moq-rs"
# Bumped by hand.
version = "0.5.0"
"""

UV_LOCK = """[[package]]
name = "moq-ffi"
version = "0.4.0"

[[package]]
name = "moq-rs"
version = "0.5.0"
source = { editable = "py/moq-rs" }
"""


class BumpTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        self.cwd = os.getcwd()
        os.chdir(self.root)
        self.git("init", "--quiet", "--initial-branch", "release")
        self.git("config", "user.name", "test")
        self.git("config", "user.email", "test@example.com")
        self.write({
            "js/a/package.json": package_json("@moq/a", "0.1.0"),
            "js/a/src/index.ts": "export {};\n",
            "js/b/package.json": package_json("@moq/b", "0.2.0"),
            "js/b/src/index.ts": "export {};\n",
            "js/wasm/package.json": package_json("@moq/wasm", "0.0.0", private=True),
            "js/wasm/src/index.ts": "export {};\n",
            "bun.lock": BUN_LOCK,
            "py/moq-rs/pyproject.toml": PYPROJECT,
            "py/moq-rs/moq/__init__.py": "",
            "uv.lock": UV_LOCK,
            "swift/VERSION": "0.3.0\n",
            "swift/Sources/Moq/Moq.swift": "",
            "kt/gradle.properties": "moqffi.version=0.0.0-dev\nmoq.version=0.4.0\n",
            "kt/moq/src/jvmMain/Moq.kt": "",
        })
        self.commit("initial")

    def tearDown(self):
        os.chdir(self.cwd)
        self.dir.cleanup()

    def git(self, *args: str) -> str:
        return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout.strip()

    def write(self, files: dict[str, str]):
        for path, text in files.items():
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(text)

    def commit(self, message: str, files: dict[str, str] | None = None) -> str:
        if files:
            self.write(files)
        self.git("add", "--all")
        self.git("commit", "--quiet", "--message", message)
        return self.git("rev-parse", "HEAD")

    def planned(self) -> dict[str, str]:
        return {b.package.name: b.new for b in bump.plan("HEAD")}

    def test_nothing_changed(self):
        self.assertEqual(self.planned(), {})

    def test_backport_bumps_only_its_package(self):
        self.commit("fix(a): a backport", {"js/a/src/index.ts": "export const fix = 1;\n"})
        self.assertEqual(self.planned(), {"@moq/a": "0.1.1"})

    def test_unshipped_files_are_not_a_release(self):
        self.commit("test(a): more tests", {
            "js/a/src/index.test.ts": "",
            "js/a/api/index.api.md": "",
            "js/a/MIGRATE.md": "",
            "kt/moq/src/jvmTest/MoqTest.kt": "",
            "swift/Tests/MoqTests/MoqTests.swift": "",
        })
        self.assertEqual(self.planned(), {})

    def test_private_package_is_skipped(self):
        self.commit("fix(wasm): private", {"js/wasm/src/index.ts": "export const fix = 1;\n"})
        self.assertEqual(self.planned(), {})

    def test_every_ecosystem(self):
        self.commit("fix: everywhere", {
            "py/moq-rs/moq/__init__.py": "fix = 1\n",
            "swift/Sources/Moq/Moq.swift": "// fix\n",
            "kt/moq/src/jvmMain/Moq.kt": "// fix\n",
        })
        self.assertEqual(self.planned(), {
            "moq-rs (Python)": "0.5.1",
            "Moq (Swift)": "0.3.1",
            "dev.moq:moq (Kotlin)": "0.4.1",
        })

    def test_bump_resets_the_baseline(self):
        self.commit("fix(a): a backport", {"js/a/src/index.ts": "export const fix = 1;\n"})
        bump.apply(bump.plan("HEAD"), self.root)
        self.commit("chore: bump")
        self.assertEqual(self.planned(), {})

    def test_cut_merge_ships_main_changes_made_after_its_bump(self):
        # On main: bump @moq/a, then land another change, then cut into release.
        self.git("checkout", "--quiet", "-b", "main")
        self.commit("chore: bump a", {"js/a/package.json": package_json("@moq/a", "0.2.0")})
        self.commit("feat(a): after the bump", {"js/a/src/index.ts": "export const feature = 1;\n"})
        self.git("checkout", "--quiet", "release")
        self.git("merge", "--quiet", "--no-ff", "--message", "cut", "main")
        # The cut's tree is what Release JS published, so nothing is pending.
        self.assertEqual(self.planned(), {})
        self.commit("fix(a): a backport", {"js/a/src/index.ts": "export const fix = 1;\n"})
        self.assertEqual(self.planned(), {"@moq/a": "0.2.1"})

    def test_apply_edits_manifest_and_lockfiles(self):
        self.commit("fix: a and python", {
            "js/a/src/index.ts": "export const fix = 1;\n",
            "py/moq-rs/moq/__init__.py": "fix = 1\n",
        })
        bump.apply(bump.plan("HEAD"), self.root)
        self.assertEqual(json.loads((self.root / "js/a/package.json").read_text())["version"], "0.1.1")
        self.assertEqual(
            (self.root / "bun.lock").read_text(),
            BUN_LOCK.replace('"version": "0.1.0"', '"version": "0.1.1"'),
        )
        self.assertIn('version = "0.5.1"', (self.root / "py/moq-rs/pyproject.toml").read_text())
        self.assertEqual(
            (self.root / "uv.lock").read_text(),
            UV_LOCK.replace('version = "0.5.0"', 'version = "0.5.1"'),
        )

    def test_apply_refuses_a_stale_lockfile(self):
        self.commit("fix(a): a backport", {
            "js/a/src/index.ts": "export const fix = 1;\n",
            "bun.lock": BUN_LOCK.replace('"version": "0.1.0"', '"version": "0.0.9"'),
        })
        with self.assertRaisesRegex(ValueError, "bun.lock does not carry @moq/a 0.1.0"):
            bump.apply(bump.plan("HEAD"), self.root)
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_patch(self):
        self.assertEqual(bump.patch("0.9.9"), "0.9.10")
        with self.assertRaises(ValueError):
            bump.patch("1.0.0-rc.1")


if __name__ == "__main__":
    unittest.main()
