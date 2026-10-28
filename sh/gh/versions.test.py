"""Check backports bump hand-versioned packages, and find the release tags to push."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("versions", Path(__file__).with_name("versions.py"))
versions = importlib.util.module_from_spec(spec)
spec.loader.exec_module(versions)


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


class VersionsTest(unittest.TestCase):
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

    def stale(self) -> dict[str, str | None]:
        """The packages at HEAD still at a released version, with what each releases alongside."""
        return {u.package.name: u.alongside for u in versions.unreleased("HEAD")}

    def backport(self, files: dict[str, str]) -> tuple[list[str], list[str]]:
        """Check a PR into `release` committing `files`, as names of the packages failing and noted."""
        self.git("checkout", "--quiet", "-B", "backport", "release")
        self.commit("fix: a backport", files)
        self.git("checkout", "--quiet", "release")
        self.problems, self.notes = versions.check("release", "backport")
        return names(self.problems), names(self.notes)

    def pending(self, assets: set[str] = frozenset()) -> dict[str, str | None]:
        with mock.patch.object(versions, "release_assets", return_value=assets):
            return {t.name: t.blocker for t in versions.pending("HEAD")}

    def pin(self, version: str) -> tuple[dict[str, str], set[str]]:
        """The files pinning the C and C++ packages to moq-ffi `version`, and its asset names."""
        ffi = versions.load_ffi()
        assets = {ffi.name(version, target) + ".tar.gz" for target in ffi.TARGETS}
        return {
            "cpp/moq/FFI_VERSION": f"# Pinned.\n{version}\n",
            "cpp/moq/FFI_SHA256.json": json.dumps({asset: "0" * 64 for asset in assets}) + "\n",
        }, assets

    def bump_a(self, version: str) -> dict[str, str]:
        return {
            "js/a/package.json": package_json("@moq/a", version),
            "bun.lock": BUN_LOCK.replace('"version": "0.1.0"', f'"version": "{version}"'),
        }

    def test_nothing_changed(self):
        self.assertEqual(self.backport({"README.md": "docs\n"}), ([], []))

    def test_backport_without_a_bump_fails(self):
        self.assertEqual(self.backport({"js/a/src/index.ts": FIX}), (["@moq/a"], []))
        self.assertIn("1 shipped file changed since 0.1.0 shipped", self.problems[0])
        self.assertIn("js/a/package.json past 0.1.0", self.problems[0])
        self.assertIn("`bun install`", self.problems[0])

    def test_backport_with_a_bump_passes(self):
        self.assertEqual(self.backport({"js/a/src/index.ts": FIX, **self.bump_a("0.1.1")}), ([], []))
        self.assertEqual(self.backport({"js/a/src/index.ts": FIX, **self.bump_a("0.2.0")}), ([], []))

    def test_a_bump_moves_its_lockfile_entry(self):
        files = {"js/a/src/index.ts": FIX, "js/a/package.json": package_json("@moq/a", "0.1.1")}
        self.assertEqual(self.backport(files), (["@moq/a"], []))
        self.assertEqual(self.problems, ["@moq/a: bun.lock carries 0.1.0, not 0.1.1; run `bun install`."])

    def test_a_lowered_version_fails(self):
        self.assertEqual(self.backport({"js/a/src/index.ts": FIX, **self.bump_a("0.0.9")}), (["@moq/a"], []))
        self.assertIn("0.0.9 does not come after 0.1.0", self.problems[0])

    def test_branch_history_is_merged_first(self):
        # The bump and the fix land in separate commits on the PR's branch.
        self.git("checkout", "--quiet", "-b", "backport")
        self.commit("chore: bump a", self.bump_a("0.1.1"))
        self.commit("fix(a): the fix", {"js/a/src/index.ts": FIX})
        self.git("checkout", "--quiet", "release")
        self.assertEqual(versions.check("release", "backport"), ([], []))

    def test_unshipped_files_are_not_a_release(self):
        self.assertEqual(self.backport({
            "js/a/src/index.test.ts": "",
            "js/a/api/index.api.md": "",
            "js/a/MIGRATE.md": "",
            "kt/moq/src/jvmTest/MoqTest.kt": "",
            "swift/Tests/MoqTests/MoqTests.swift": "",
            "dart/moq/test/moq_test.dart": "",
        }), ([], []))

    def test_private_package_is_skipped(self):
        self.assertEqual(self.backport({"js/wasm/src/index.ts": FIX}), ([], []))

    def test_every_ecosystem(self):
        failing, _ = self.backport({
            "py/moq-rs/moq/__init__.py": "fix = 1\n",
            "swift/Sources/Moq/Moq.swift": "// fix\n",
            "kt/moq/src/jvmMain/Moq.kt": "// fix\n",
        })
        self.assertEqual(failing, ["moq-rs (Python)", "Moq (Swift)", "dev.moq:moq (Kotlin)"])
        self.assertIn("`uv lock`", self.problems[0])
        self.assertEqual(self.backport({
            "py/moq-rs/moq/__init__.py": "fix = 1\n",
            "py/moq-rs/pyproject.toml": PYPROJECT.replace('"0.5.0"', '"0.5.1"'),
            "uv.lock": UV_LOCK.replace('version = "0.5.0"', 'version = "0.5.1"'),
            "swift/Sources/Moq/Moq.swift": "// fix\n",
            "swift/VERSION": "0.3.1\n",
            "kt/moq/src/jvmMain/Moq.kt": "// fix\n",
            "kt/gradle.properties": "moqffi.version=0.0.0-dev\nmoq.version=0.4.1\n",
        }), ([], []))

    def test_unreleased_changes_already_on_base_are_only_noted(self):
        self.commit("fix(b): merged without a bump", {"js/b/src/index.ts": FIX})
        self.assertEqual(self.backport({"js/a/src/index.test.ts": ""}), ([], ["@moq/b"]))
        # A backport touching it must bump it, which ships the earlier change too.
        self.assertEqual(self.backport({"js/b/src/other.ts": FIX}), (["@moq/b"], []))
        self.assertIn("2 shipped files changed", self.problems[0])

    def test_cut_merge_ships_main_changes_made_after_its_bump(self):
        # On main: bump @moq/a, then land another change, then cut into release.
        self.git("checkout", "--quiet", "-b", "main")
        self.commit("chore: bump a", self.bump_a("0.2.0"))
        self.commit("feat(a): after the bump", {"js/a/src/index.ts": "export const feature = 1;\n"})
        self.git("checkout", "--quiet", "release")
        self.git("merge", "--quiet", "--no-ff", "--message", "cut", "main")
        # The cut's tree is what Release JS published, so nothing is unreleased.
        self.assertEqual(self.stale(), {})
        self.assertEqual(self.backport({"js/a/src/index.ts": FIX}), (["@moq/a"], []))
        self.assertIn("past 0.2.0", self.problems[0])

    def test_tagged_package_needs_a_bump_after_its_tag(self):
        self.assertEqual(self.backport({"dart/moq/lib/moq.dart": "// fix\n"}), (["moq (Dart)"], []))
        self.assertIn("dart/moq/pubspec.yaml past 0.1.0", self.problems[0])

    def test_untagged_version_needs_no_bump(self):
        self.commit("chore: bump dart", {"dart/moq/pubspec.yaml": "name: moq\nversion: 0.1.1\n"})
        self.assertEqual(self.backport({"dart/moq/lib/moq.dart": "// fix\n"}), ([], []))

    def test_cpp_and_obs_release_together(self):
        self.assertEqual(self.backport({"cpp/moq/src/moq.cpp": "// fix\n"}), (["moq (C++)", "obs-moq (OBS)"], []))
        self.assertEqual(self.backport({"cpp/moq/src/moq.cpp": "// fix\n", "cpp/moq/VERSION": "0.1.1\n"}),
                         (["obs-moq (OBS)"], []))
        self.assertIn("moq (C++) releases, and the same tag would publish its released 0.7.0 again", self.problems[0])
        self.assertEqual(self.backport({"cpp/obs/src/plugin.cpp": "// fix\n", "cpp/obs/VERSION": "0.7.1\n"}),
                         (["moq (C++)"], []))
        self.assertEqual(self.backport({
            "cpp/obs/src/plugin.cpp": "// fix\n",
            "cpp/moq/VERSION": "0.1.1\n",
            "cpp/obs/VERSION": "0.7.1\n",
        }), ([], []))

    def test_ffi_pin_change_releases_c_and_cpp(self):
        files, _ = self.pin("0.4.20")
        self.assertEqual(self.backport(files), (["moq (C)", "moq (C++)", "obs-moq (OBS)"], []))

    def test_untagged_version_awaits_its_tag(self):
        self.commit("chore: bump dart", {"dart/moq/pubspec.yaml": "name: moq\nversion: 0.1.1\n"})
        self.commit("fix(dart): after the bump", {"dart/moq/lib/moq.dart": "// fix\n"})
        self.assertEqual(self.stale(), {})
        self.assertEqual(self.pending(), {"moq-dart-v0.1.1": None})

    def test_untagged_cpp_waits_for_the_obs_bump(self):
        files, assets = self.pin("0.4.20")
        self.commit("chore: pin moq-ffi and bump", {**files, "c/moq/VERSION": "0.8.1\n", "cpp/moq/VERSION": "0.2.0\n"})
        self.assertEqual(self.stale(), {"obs-moq (OBS)": "moq (C++)"})
        # Tagging now would publish OBS 0.7.0 again.
        self.assertIn("obs-moq (OBS) 0.7.0 is already released", self.pending(assets)["cpp-v0.2.0"])
        self.commit("chore: bump obs", {"cpp/obs/VERSION": "0.7.1\n"})
        self.assertEqual(self.pending(assets), {"c-v0.8.1": None, "cpp-v0.2.0": None})

    def test_obs_in_flight_from_a_cpp_tag_is_released(self):
        self.commit("chore: bump", {"cpp/moq/VERSION": "0.1.1\n", "cpp/obs/VERSION": "0.7.1\n"})
        # Release Cpp has not cut obs-moq-v0.7.1 yet.
        self.git("tag", "cpp-v0.1.1")
        self.commit("docs: unrelated", {"README.md": ""})
        self.assertEqual(self.stale(), {})
        self.assertEqual(self.pending(), {})
        self.assertEqual(self.backport({"cpp/obs/src/plugin.cpp": "// fix\n"}), (["moq (C++)", "obs-moq (OBS)"], []))

    def test_obs_never_pushes_its_own_tag(self):
        self.commit("chore: bump obs", {"cpp/obs/VERSION": "0.8.0\n"})
        self.assertEqual(self.stale(), {"moq (C++)": "obs-moq (OBS)"})
        self.assertEqual(self.pending(), {})

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
        files, assets = self.pin("0.4.20")
        self.commit("chore: pin moq-ffi", files)
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

        def pushed() -> str:
            return subprocess.run(["git", "ls-remote", "--tags", str(remote)], capture_output=True, text=True, check=True).stdout

        with mock.patch.object(versions, "release_assets", return_value=set()):
            versions.push_tags(dry_run=True)
            self.assertEqual(pushed(), "")
            versions.push_tags()
        self.assertEqual(pushed(), f"{head}\trefs/tags/moq-dart-v0.1.1\n")

    def test_missing_tagged_packages_are_skipped(self):
        # A `release` before the cut that brings C, C++, and OBS.
        self.git("rm", "--quiet", "-r", "c", "cpp")
        self.commit("chore: drop c and cpp")
        # That `release` lacks sh/cpp/ffi.py too.
        with mock.patch.object(versions, "load_ffi", side_effect=AssertionError("loaded ffi.py")):
            self.assertEqual(self.stale(), {})
            self.assertEqual(self.pending(), {})
            self.assertEqual(self.backport({"js/a/src/index.ts": FIX}), (["@moq/a"], []), self.problems)

    def test_newer(self):
        self.assertTrue(versions.newer("0.9.10", "0.9.9"))
        self.assertTrue(versions.newer("0.1.0", "0.1.0-rc.1"))
        self.assertFalse(versions.newer("0.1.0-rc.1", "0.1.0"))
        self.assertFalse(versions.newer("0.1.0", "0.1.0"))
        self.assertIsNone(versions.newer("next", "0.1.0"))


def names(lines: list[str]) -> list[str]:
    return [line.split(": ", 1)[0] for line in lines]


FIX = "export const fix = 1;\n"


if __name__ == "__main__":
    unittest.main()
