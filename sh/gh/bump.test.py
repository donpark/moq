"""Find hand-versioned packages with unreleased changes on a first-parent history."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

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
            "dart/moq/pubspec.yaml": "name: moq\nversion: 0.1.0\n",
            "dart/moq/lib/moq.dart": "",
            "c/moq/VERSION": "0.8.0\n",
            "c/moq/src/moq.c": "",
            "cpp/moq/VERSION": "0.1.0\n",
            "cpp/moq/src/moq.cpp": "",
            "cpp/moq/FFI_VERSION": "# Unset until published.\n",
            "cpp/moq/FFI_SHA256.json": "{}\n",
            "cpp/obs/VERSION": "0.7.0\n",
            "cpp/obs/src/plugin.cpp": "",
        })
        self.commit("initial")
        # Every tag-published package has released its current version.
        for tag in ("moq-dart-v0.1.0", "c-v0.8.0", "cpp-v0.1.0", "obs-moq-v0.7.0"):
            self.git("tag", tag)

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

    def pending(self, assets: set[str] = frozenset()) -> dict[str, str | None]:
        with mock.patch.object(bump, "release_assets", return_value=assets):
            return {t.name: t.blocker for t in bump.pending("HEAD")}

    def pin(self, version: str) -> set[str]:
        """Pin the C and C++ packages to moq-ffi `version`, returning its asset names."""
        ffi = bump.load_ffi()
        assets = {ffi.name(version, target) + ".tar.gz" for target in ffi.TARGETS}
        self.commit("chore: pin moq-ffi", {
            "cpp/moq/FFI_VERSION": f"# Pinned.\n{version}\n",
            "cpp/moq/FFI_SHA256.json": json.dumps({asset: "0" * 64 for asset in assets}) + "\n",
        })
        return assets

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

    def test_tagged_package_bumps_after_a_change_since_its_tag(self):
        self.commit("test(dart): more tests", {"dart/moq/test/moq_test.dart": ""})
        self.assertEqual(self.planned(), {})
        self.commit("fix(dart): a backport", {"dart/moq/lib/moq.dart": "// fix\n"})
        self.assertEqual(self.planned(), {"moq (Dart)": "0.1.1"})
        self.assertEqual(self.pending(), {})

    def test_untagged_version_awaits_its_tag_instead_of_a_bump(self):
        self.commit("chore: bump dart", {"dart/moq/pubspec.yaml": "name: moq\nversion: 0.1.1\n"})
        self.commit("fix(dart): after the bump", {"dart/moq/lib/moq.dart": "// fix\n"})
        self.assertEqual(self.planned(), {})
        self.assertEqual(self.pending(), {"moq-dart-v0.1.1": None})

    def test_cpp_and_obs_release_together(self):
        self.commit("fix(obs): plugin only", {"cpp/obs/src/plugin.cpp": "// fix\n"})
        self.assertEqual(self.planned(), {"moq (C++)": "0.1.1", "obs-moq (OBS)": "0.7.1"})
        alongside = {b.package.name: b.alongside for b in bump.plan("HEAD")}
        self.assertEqual(alongside, {"moq (C++)": "obs-moq (OBS)", "obs-moq (OBS)": None})

    def test_cpp_release_bumps_obs(self):
        self.commit("fix(cpp): library only", {"cpp/moq/src/moq.cpp": "// fix\n"})
        self.assertEqual(self.planned(), {"moq (C++)": "0.1.1", "obs-moq (OBS)": "0.7.1"})

    def test_untagged_cpp_bumps_a_released_obs_before_its_tag(self):
        assets = self.pin("0.4.20")
        self.commit("chore: bump cpp", {"cpp/moq/VERSION": "0.2.0\n"})
        self.assertEqual(self.planned(), {"moq (C)": "0.8.1", "obs-moq (OBS)": "0.7.1"})
        # Tagging now would publish OBS 0.7.0 again.
        self.assertIn("obs-moq (OBS) 0.7.0 is already released", self.pending(assets)["cpp-v0.2.0"])
        bump.apply(bump.plan("HEAD"), self.root)
        self.commit("chore: bump")
        self.assertEqual(self.pending(assets), {"c-v0.8.1": None, "cpp-v0.2.0": None})

    def test_obs_never_pushes_its_own_tag(self):
        self.commit("chore: bump obs", {"cpp/obs/VERSION": "0.8.0\n"})
        self.assertEqual(self.planned(), {"moq (C++)": "0.1.1"})
        self.assertEqual(self.pending(), {})

    def test_ffi_pin_change_releases_c_and_cpp(self):
        self.pin("0.4.20")
        self.assertEqual(self.planned(), {"moq (C)": "0.8.1", "moq (C++)": "0.1.1", "obs-moq (OBS)": "0.7.1"})

    def test_c_and_cpp_tags_wait_for_the_pinned_ffi_release(self):
        self.commit("chore: bump", {
            "dart/moq/pubspec.yaml": "name: moq\nversion: 0.1.1\n",
            "c/moq/VERSION": "0.8.1\n",
            "cpp/moq/VERSION": "0.1.1\n",
            "cpp/obs/VERSION": "0.7.1\n",
        })
        self.assertEqual(self.pending(), {
            "moq-dart-v0.1.1": None,
            "c-v0.8.1": "cpp/moq/FFI_VERSION pins no single moq-ffi release",
            "cpp-v0.1.1": "cpp/moq/FFI_VERSION pins no single moq-ffi release",
        })
        assets = self.pin("0.4.20")
        missing = sorted(assets)[-1]
        blocked = self.pending(assets - {missing})
        self.assertIn(missing, blocked["c-v0.8.1"])
        self.assertEqual(self.pending(assets), {"moq-dart-v0.1.1": None, "c-v0.8.1": None, "cpp-v0.1.1": None})

    def test_tag_pushes_only_unblocked_tags(self):
        remote_dir = tempfile.TemporaryDirectory()
        self.addCleanup(remote_dir.cleanup)
        remote = Path(remote_dir.name)
        subprocess.run(["git", "init", "--quiet", "--bare", str(remote)], check=True)
        self.git("remote", "add", "origin", str(remote))
        head = self.commit("chore: bump", {"dart/moq/pubspec.yaml": "name: moq\nversion: 0.1.1\n", "c/moq/VERSION": "0.8.1\n"})
        with mock.patch.object(bump, "release_assets", return_value=set()):
            bump.push_tags()
        pushed = subprocess.run(["git", "ls-remote", "--tags", str(remote)], capture_output=True, text=True, check=True)
        self.assertEqual(pushed.stdout, f"{head}\trefs/tags/moq-dart-v0.1.1\n")

    def test_missing_tagged_packages_are_skipped(self):
        # A `release` before the cut that brings C, C++, and OBS.
        self.git("rm", "--quiet", "-r", "c", "cpp")
        self.commit("chore: drop c and cpp")
        # That `release` lacks sh/cpp/ffi.py too.
        with mock.patch.object(bump, "load_ffi", side_effect=AssertionError("loaded ffi.py")):
            self.assertEqual(self.planned(), {})
            self.assertEqual(self.pending(), {})

    def test_patch(self):
        self.assertEqual(bump.patch("0.9.9"), "0.9.10")
        with self.assertRaises(ValueError):
            bump.patch("1.0.0-rc.1")


if __name__ == "__main__":
    unittest.main()
