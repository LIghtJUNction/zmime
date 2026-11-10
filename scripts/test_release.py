"""Offline release regression tests: no token or network access is required."""

import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release


MANIFEST = '[package]\nname = "zmime"\nversion = "0.1.0"\nrust-version = "1.74"\n\n[dependencies]\n'
LOCK = '# generated\nversion = 3\n\n[[package]]\nname = "zmime"\nversion = "0.1.0"\n'


class VersionTests(unittest.TestCase):
    def test_all_increments(self):
        for mode, expected in (("none", "1.9.9"), ("patch", "1.9.10"), ("minor", "1.10.0"), ("major", "2.0.0")):
            self.assertEqual(release.version_bump("1.9.9", mode), expected)

    def test_invalid_versions_rejected(self):
        for version in ("01.2.3", "1.2", "1.2.3-rc1", "1.2.3\n", "1.2.3+build", "-1.2.3"):
            with self.subTest(version=version), self.assertRaises(RuntimeError):
                release.version_bump(version, "patch")
        with self.assertRaises(RuntimeError):
            release.version_bump("1.2.3", "invalid")

    def test_manifest_rewrite_is_scoped(self):
        manifest = MANIFEST + 'other = { version = "0.1.0" }\n'
        updated = release.replace_manifest_version(manifest, "0.1.0", "0.2.0")
        self.assertIn('version = "0.2.0"', updated)
        self.assertIn('other = { version = "0.1.0" }', updated)

    def test_manifest_mismatch_fails(self):
        with self.assertRaises(RuntimeError):
            release.replace_manifest_version(MANIFEST, "0.9.0", "0.9.1")

    def test_lockfile_does_not_change_registry_dependency(self):
        text = LOCK + '\n[[package]]\nname = "other"\nversion = "0.1.0"\nsource = "registry+test"\n'
        updated = release.replace_lock_version(text, "zmime", "0.1.0", "0.2.0")
        self.assertIn('name = "zmime"\nversion = "0.2.0"', updated)
        self.assertIn('name = "other"\nversion = "0.1.0"', updated)

    def test_registry_zmime_entry_cannot_replace_path_entry(self):
        with self.assertRaises(RuntimeError):
            release.replace_lock_version(LOCK + 'source = "registry+test"\n', "zmime", "0.1.0", "0.2.0")

    def test_duplicate_path_entries_fail(self):
        duplicate = LOCK + "\n[[package]]" + LOCK.split("[[package]]", 1)[1]
        with self.assertRaises(RuntimeError):
            release.replace_lock_version(duplicate, "zmime", "0.1.0", "0.2.0")


class RegistryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.archive = Path(self.temp.name) / "zmime.crate"
        self.archive.write_bytes(b"verified archive")
        self.record = {"cksum": hashlib.sha256(self.archive.read_bytes()).hexdigest(), "yanked": False}

    def test_existing_identical_archive_is_idempotent(self):
        self.assertTrue(release.check_registry(self.archive, self.record))

    def test_missing_version_can_publish(self):
        self.assertFalse(release.check_registry(self.archive, None))

    def test_existing_different_archive_fails_closed(self):
        self.archive.write_bytes(b"different archive")
        with self.assertRaises(RuntimeError):
            release.check_registry(self.archive, self.record)

    def test_yanked_version_is_never_reused(self):
        self.record["yanked"] = True
        with self.assertRaises(RuntimeError):
            release.check_registry(self.archive, self.record)


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "checkout"
        self.root.mkdir()
        self.remote = self.base / "remote.git"
        self.shell("git", "init", "--bare", str(self.remote))
        self.shell("git", "init", "-b", "main")
        self.shell("git", "config", "user.name", "Release Test")
        self.shell("git", "config", "user.email", "test@example.invalid")
        (self.root / "Cargo.toml").write_text(MANIFEST)
        (self.root / "Cargo.lock").write_text(LOCK)
        (self.root / ".gitignore").write_text("/target/\n")
        (self.root / "comparison").mkdir()
        (self.root / "comparison/Cargo.lock").write_text(LOCK)
        self.shell("git", "add", ".")
        self.shell("git", "commit", "-m", "Initial crate")
        self.source = self.shell("git", "rev-parse", "HEAD")
        self.shell("git", "remote", "add", "origin", str(self.remote))
        self.shell("git", "push", "origin", "main")
        self.env = {
            "RELEASE_BRANCH": "main", "RELEASE_BUMP": "patch", "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_REF": "refs/heads/main", "GITHUB_SHA": self.source,
            "GITHUB_REPOSITORY": "example/zmime", "GITHUB_RUN_ID": "123",
            "GITHUB_OUTPUT": str(self.base / "outputs"),
        }
        self.root_patch = patch.object(release, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.env_patch = patch.dict(os.environ, self.env)
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)

    def shell(self, *args):
        return subprocess.check_output(args, cwd=self.root, text=True, stderr=subprocess.DEVNULL).strip()

    def test_bump_commit_and_retry_share_identical_sha(self):
        release.prepare()
        first = self.shell("git", "rev-parse", "HEAD")
        self.assertNotEqual(first, self.source)
        self.assertEqual(release.package()["version"], "0.1.1")
        self.assertIn('version = "0.1.1"', (self.root / "Cargo.lock").read_text())
        self.assertIn('version = "0.1.1"', (self.root / "comparison/Cargo.lock").read_text())
        release.prepare()
        self.assertEqual(self.shell("git", "rev-parse", "HEAD"), first)
        self.assertEqual(self.shell("git", "rev-parse", "origin/main"), first)

    def test_none_publishes_current_version_without_mutation(self):
        os.environ["RELEASE_BUMP"] = "none"
        release.prepare()
        self.assertEqual(self.shell("git", "rev-parse", "HEAD"), self.source)
        self.assertEqual(release.package()["version"], "0.1.0")

    def test_minor_and_major_update_manifest_both_locks_and_remote(self):
        for run_id, mode, version in (("124", "minor", "0.2.0"), ("125", "major", "1.0.0")):
            with self.subTest(mode=mode):
                previous = self.shell("git", "rev-parse", "HEAD")
                os.environ.update({
                    "GITHUB_SHA": previous, "GITHUB_RUN_ID": run_id, "RELEASE_BUMP": mode,
                })
                release.prepare()
                prepared = self.shell("git", "rev-parse", "HEAD")
                self.assertNotEqual(prepared, previous)
                self.assertEqual(release.package()["version"], version)
                for path in ("Cargo.lock", "comparison/Cargo.lock"):
                    locked = release.tomllib.loads((self.root / path).read_text())
                    self.assertEqual(locked["package"][0]["version"], version)
                self.assertEqual(self.shell("git", "rev-parse", "origin/main"), prepared)
                self.assertEqual(self.shell("git", "rev-parse", prepared + "^"), previous)
                release.prepare()
                self.assertEqual(self.shell("git", "rev-parse", "HEAD"), prepared)

    def test_stale_dispatch_rejected(self):
        (self.root / "new.txt").write_text("new work")
        self.shell("git", "add", ".")
        self.shell("git", "commit", "-m", "A newer commit")
        self.shell("git", "push", "origin", "main")
        with self.assertRaisesRegex(RuntimeError, "advanced"):
            release.prepare()

    def test_other_branch_rejected(self):
        os.environ["GITHUB_REF"] = "refs/heads/untrusted"
        with self.assertRaisesRegex(RuntimeError, "default branch"):
            release.prepare()

    def test_release_event_is_not_implicitly_trusted(self):
        os.environ["GITHUB_EVENT_NAME"] = "release"
        with self.assertRaisesRegex(RuntimeError, "manual default-branch"):
            release.prepare()

    def test_dirty_checkout_rejected(self):
        (self.root / "Cargo.toml").write_text(MANIFEST + "# dirty\n")
        with self.assertRaisesRegex(RuntimeError, "clean"):
            release.prepare()

    def test_existing_tag_cannot_be_overwritten(self):
        self.shell("git", "tag", "v0.1.1")
        self.shell("git", "push", "origin", "v0.1.1")
        with self.assertRaisesRegex(RuntimeError, "already has a tag"):
            release.prepare()
        self.assertEqual(self.shell("git", "rev-parse", "HEAD"), self.source)

    def test_existing_non_commit_tag_is_rejected_before_bump(self):
        blob = self.shell("git", "hash-object", "-w", "Cargo.toml")
        self.shell("git", "tag", "v0.1.1", blob)
        self.shell("git", "push", "origin", "refs/tags/v0.1.1")
        with self.assertRaisesRegex(RuntimeError, "does not refer to a commit"):
            release.prepare()
        self.assertEqual(self.shell("git", "rev-parse", "HEAD"), self.source)
        self.assertEqual(release.package()["version"], "0.1.0")

    def test_identical_tag_can_finish_current_release(self):
        os.environ["RELEASE_BUMP"] = "none"
        self.shell("git", "tag", "v0.1.0")
        self.shell("git", "push", "origin", "v0.1.0")
        release.prepare()
        self.assertEqual(self.shell("git", "rev-parse", "HEAD"), self.source)

    def test_lock_mismatch_does_not_push_partial_release(self):
        (self.root / "Cargo.lock").write_text(LOCK.replace("0.1.0", "0.9.0"))
        self.shell("git", "add", ".")
        self.shell("git", "commit", "-m", "Inconsistent lockfile")
        source = self.shell("git", "rev-parse", "HEAD")
        os.environ["GITHUB_SHA"] = source
        self.shell("git", "push", "origin", "main")
        with self.assertRaisesRegex(RuntimeError, "disagrees"):
            release.prepare()
        self.assertEqual(self.shell("git", "rev-parse", "origin/main"), source)

    def packaged_release(self):
        os.environ.update({
            "RELEASE_SHA": self.source, "RELEASE_VERSION": "0.1.0",
            "RELEASE_TAG": "v0.1.0", "GH_TOKEN": "offline-test-placeholder",
        })
        archive = self.root / "target/package/zmime-0.1.0.crate"
        archive.parent.mkdir(parents=True)
        archive.write_bytes(b"tested package")
        return {"cksum": hashlib.sha256(archive.read_bytes()).hexdigest(), "yanked": False}

    def test_unpublished_crate_never_creates_tag_or_release(self):
        self.packaged_release()
        with patch.object(release, "registry_record", return_value=None), \
                patch.object(release.time, "sleep"), \
                patch.object(release, "fetch_json") as api:
            with self.assertRaisesRegex(RuntimeError, "not visible"):
                release.finalize()
            api.assert_not_called()
        self.assertIsNone(release.tag_target("v0.1.0"))

    def test_registry_checksum_conflict_prevents_tag(self):
        record = self.packaged_release()
        record["cksum"] = "0" * 64
        with patch.object(release, "registry_record", return_value=record):
            with self.assertRaisesRegex(RuntimeError, "different archive"):
                release.finalize()
        self.assertIsNone(release.tag_target("v0.1.0"))

    def test_tag_and_github_release_created_only_after_registry_verification(self):
        record = self.packaged_release()
        created = {"html_url": "https://github.com/example/zmime/releases/tag/v0.1.0"}
        with patch.object(release, "registry_record", return_value=record), \
                patch.object(release, "fetch_json", side_effect=[None, created]) as api:
            release.finalize()
            self.assertEqual(api.call_count, 2)
            self.assertEqual(api.call_args.kwargs["method"], "POST")
            self.assertEqual(api.call_args.kwargs["payload"]["target_commitish"], self.source)
        self.assertEqual(release.tag_target("v0.1.0"), self.source)
        self.assertIn("refs/tags/v0.1.0", self.shell("git", "ls-remote", "--tags", "origin"))

    def test_finished_release_retry_has_no_write(self):
        record = self.packaged_release()
        self.shell("git", "tag", "v0.1.0")
        self.shell("git", "push", "origin", "v0.1.0")
        existing = {"id": 7, "html_url": "https://github.com/example/zmime/releases/tag/v0.1.0",
                    "tag_name": "v0.1.0", "draft": False, "prerelease": False}
        with patch.object(release, "registry_record", return_value=record), \
                patch.object(release, "fetch_json", return_value=existing) as api:
            release.finalize()
            api.assert_called_once()
            self.assertNotIn("method", api.call_args.kwargs)

    def test_draft_release_recovery(self):
        record = self.packaged_release()
        existing = {"id": 7, "html_url": "https://github.com/example/zmime/releases/tag/v0.1.0",
                    "tag_name": "v0.1.0", "draft": True, "prerelease": False}
        with patch.object(release, "registry_record", return_value=record), \
                patch.object(release, "fetch_json", return_value=existing) as api:
            release.finalize()
            self.assertEqual(api.call_args.kwargs["method"], "PATCH")
            self.assertEqual(api.call_args.kwargs["payload"], {"draft": False})


if __name__ == "__main__":
    unittest.main()
