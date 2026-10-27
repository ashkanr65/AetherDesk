"""Disposable publisher fixtures. No real network, signing key, push or release."""
import copy
from contextlib import redirect_stdout
import hashlib
import io
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import urllib.error
import zipfile

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
import auto_update as auto


class FakeGit:
    def __init__(self):
        self.commit = "a" * 40
        self.pushes = 0
        self.branch = "main"
        self.dirty = []

    def validate(self, repository):
        auto.require(self.branch == "main", "Switch to main")

    def audit(self):
        return self.dirty

    def pending(self):
        return self.dirty

    def head(self):
        return self.commit

    def push(self, repository, tag, checked_inputs):
        self.validate(repository)
        checked_inputs()
        self.pushes += 1
        self.commit = "b" * 40
        self.dirty = []
        return self.commit


class FakeGitHub(auto.GitHub):
    def __init__(self, git):
        self.git = git
        self.prefix = "/repos/example/AetherDesk"
        self.release = None
        self.data = {}
        self.creates = 0
        self.uploads = 0
        self.publishes = 0
        self.private = False
        self.default_branch = "main"
        self.interrupt_upload = False
        self.on_upload = None
        self.wrong_tag = False
        self.newer = None
        self.tamper_public = False

    def find(self, tag):
        return copy.deepcopy(self.release)

    def api(self, path, method="GET", body=None, missing=False):
        if path == self.prefix:
            return {"private": self.private, "default_branch": self.default_branch, "archived": False}
        if path.endswith("/releases/latest"):
            return copy.deepcopy(self.newer or (self.release if self.release and not self.release["draft"] else None))
        if path.endswith("/git/ref/heads/main"):
            return {"object": {"sha": self.git.head()}}
        if "/git/ref/tags/" in path:
            if self.wrong_tag:
                return {"object": {"type": "commit", "sha": "c" * 40}}
            return {"object": {"type": "commit", "sha": self.git.head()}} if self.release and not self.release["draft"] else None
        if path == self.prefix + "/releases" and method == "POST":
            self.creates += 1
            self.release = {**body, "id": 11, "assets": [], "html_url": "https://example.invalid/release"}
        elif path.endswith("/releases/11") and method == "PATCH":
            self.publishes += 1
            self.release.update(body)
        elif "/releases/assets/" in path and method == "DELETE":
            self.release["assets"] = [a for a in self.release["assets"] if a["id"] != int(path.rsplit("/", 1)[1])]
        elif not path.endswith("/releases/11"):
            raise AssertionError("Unexpected fixture API route: " + method + " " + path)
        return copy.deepcopy(self.release)

    def upload(self, release_id, path, log=None):
        self.uploads += 1
        data = path.read_bytes()
        identifier = 100 + len(self.data)
        self.data[identifier] = data
        self.release["assets"].append({"id": identifier, "name": path.name, "size": len(data),
            "state": "uploaded", "digest": "sha256:" + hashlib.sha256(data).hexdigest()})
        if self.on_upload:
            self.on_upload()
        if self.interrupt_upload:
            self.interrupt_upload = False
            raise auto.Stop("Simulated lost response after server accepted upload")

    def anonymous(self, path, binary=False):
        if path.endswith("/releases/latest"):
            return io.BytesIO(json.dumps(self.release).encode())
        identifier = int(path.rsplit("/", 1)[1])
        data = self.data[identifier]
        if self.tamper_public:
            data = data[:-1] + bytes([data[-1] ^ 1])
        return io.BytesIO(data)


class PublisherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aetherdesk-publisher-fixture-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "source"
        self.output = self.base / "output"
        self.package = self.output / "packages/fixture"
        self.root.mkdir()
        (self.package / "verification").mkdir(parents=True)
        (self.package / "agent/linux-x86_64").mkdir(parents=True)
        (self.output / "reports").mkdir()
        self.key = Ed25519PrivateKey.generate()
        self.write(self.root / "update-config.json", {"repository": "example/AetherDesk", "publicKey": auto.signing.public(self.key)})
        release = {"version": "1.2.3", "build": 4}
        self.write(self.root / "release.json", release)
        self.write(self.package / "release.json", release)
        (self.root / "RELEASE-NOTES.md").write_text("# AetherDesk 1.2.3 - build 4\n\nFixture release.\n", encoding="utf-8")
        self.write(self.root / "rust-regressions.json", {})
        self.write(self.package / "verification/regression-catalog.json", {})
        evidence = {"schemaVersion": 1, "compilerTestsPassed": True, "required": 1, "registered": 1,
                    "catalogSha256": auto.signing.sha(self.root / "rust-regressions.json")}
        self.write(self.package / "verification/windows-regressions.json", evidence)
        self.write(self.package / "agent/linux-x86_64/regression-results.json", evidence)
        self.write(self.package / "verification/x11-runtime.json", {"bundledX11TestPassed": True})
        self.write(self.package / "build-info.json", {**release, "runtime": "native-rust", "fingerprint": "a" * 64,
                                                    "built": "2025-01-02T00:00:10+00:00"})
        self.write(self.output / "reports/build-timing.json", {"status": "Passed", "startedUtc": "2025-01-02T00:00:00+00:00",
                   "updatedUtc": "2025-01-02T00:01:00+00:00", "steps": [{"step": n, "status": "Passed"} for n in range(7)]})
        image = bytearray(80)
        image[:2] = b"MZ"
        struct.pack_into("<I", image, 60, 64)
        image[64:70] = b"PE\0\0\x64\x86"
        (self.package / "AetherDesk.exe").write_bytes(image)
        self.installer = self.output / "packages/fixture-Setup.exe"
        self.installer.write_bytes(b"installer fixture")
        (self.output / "LATEST-FOLDER.txt").write_text(str(self.package), encoding="utf-8-sig")
        (self.output / "LATEST-INSTALLER.txt").write_text(str(self.installer), encoding="utf-8-sig")
        original = self.output / "packages/fixture-original.zip"
        with zipfile.ZipFile(original, "w") as archive:
            for path in self.package.rglob("*"):
                if path.is_file():
                    archive.write(path, self.package.name + "/" + path.relative_to(self.package).as_posix())
        (self.output / "LATEST-ZIP.txt").write_text(str(original), encoding="utf-8-sig")
        self.git = FakeGit()
        self.github = FakeGitHub(self.git)
        self.patches = [patch.object(auto, "fingerprint", return_value="a" * 64),
                        patch.object(auto, "file_version", return_value=(1, 2, 3, 4)),
                        patch.object(auto.signing, "load_key", return_value=self.key),
                        patch.object(auto, "GitHub", return_value=self.github)]
        for item in self.patches:
            item.start()
            self.addCleanup(item.stop)

    @staticmethod
    def write(path, value):
        path.write_text(json.dumps(value) + "\n", encoding="utf-8")

    def publisher(self, **kwargs):
        publisher = auto.Publisher(root=self.root, output=self.output, **kwargs)
        publisher.git = self.git
        return publisher

    def run_quiet(self, publisher):
        with redirect_stdout(io.StringIO()):
            return publisher.run()

    def test_full_release_verifies_downloads_and_rerun_does_not_publish_duplicate(self):
        self.assertEqual(self.run_quiet(self.publisher()), "https://example.invalid/release")
        self.assertEqual((self.git.pushes, self.github.creates, self.github.uploads, self.github.publishes), (1, 1, 3, 1))
        self.run_quiet(self.publisher())
        self.assertEqual((self.git.pushes, self.github.creates, self.github.uploads, self.github.publishes), (1, 1, 3, 1))
        report = auto.read_json(self.output / "acceptance/auto-update-v1.2.3-build4/run.json")
        self.assertEqual(report["status"], "Published and verified")
        self.assertFalse(report["actual_gui_upgrade_tested"])

    def test_draft_is_resumable_after_upload_response_loss(self):
        self.github.interrupt_upload = True
        with self.assertRaises(auto.Stop):
            self.run_quiet(self.publisher())
        self.assertTrue(self.github.release["draft"])
        self.run_quiet(self.publisher())
        self.assertEqual((self.github.creates, self.github.uploads, self.github.publishes), (1, 3, 1))

    def test_check_mode_does_not_access_keys_credentials_network_or_git_writes(self):
        with patch.object(auto.signing, "load_key", side_effect=AssertionError("key accessed")), \
             patch.object(auto, "GitHub", side_effect=AssertionError("network accessed")):
            self.assertIsNone(self.run_quiet(self.publisher(check=True)))
        self.assertEqual(self.git.pushes, 0)
        self.assertFalse((self.output / "releases").exists())

    def test_draft_mode_uploads_but_does_not_enable_stable_updates(self):
        self.run_quiet(self.publisher(draft=True))
        self.assertTrue(self.github.release["draft"])
        self.assertEqual(self.github.publishes, 0)
        self.run_quiet(self.publisher())
        self.assertEqual(self.github.publishes, 1)

    def test_stale_build_failed_stage_and_empty_test_evidence_stop_before_push(self):
        with patch.object(auto, "fingerprint", return_value="c" * 64):
            with self.assertRaisesRegex(auto.Stop, "Source changed"):
                self.run_quiet(self.publisher())
        timing = auto.read_json(self.output / "reports/build-timing.json")
        timing["steps"][2]["status"] = "Failed"
        self.write(self.output / "reports/build-timing.json", timing)
        with self.assertRaisesRegex(auto.Stop, "stages"):
            self.run_quiet(self.publisher())
        timing["steps"][2]["status"] = "Passed"
        self.write(self.output / "reports/build-timing.json", timing)
        evidence = auto.read_json(self.package / "verification/windows-regressions.json")
        evidence["required"] = 0
        self.write(self.package / "verification/windows-regressions.json", evidence)
        with self.assertRaisesRegex(auto.Stop, "empty"):
            self.run_quiet(self.publisher())
        self.assertEqual(self.git.pushes, 0)

    def test_private_repository_wrong_branch_key_or_tag_never_publish(self):
        self.github.private = True
        with self.assertRaisesRegex(auto.Stop, "public"):
            self.run_quiet(self.publisher())
        self.github.private = False
        self.git.branch = "other"
        with self.assertRaisesRegex(auto.Stop, "main"):
            self.run_quiet(self.publisher())
        self.git.branch = "main"
        with patch.object(auto.signing, "load_key", return_value=Ed25519PrivateKey.generate()):
            with self.assertRaisesRegex(auto.Stop, "key"):
                self.run_quiet(self.publisher())
        self.github.wrong_tag = True
        with self.assertRaisesRegex(auto.Stop, "tag"):
            self.run_quiet(self.publisher())
        self.assertEqual(self.github.publishes, 0)

    def test_source_notes_change_during_upload_keeps_draft(self):
        self.github.on_upload = lambda: (self.root / "RELEASE-NOTES.md").write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(auto.Stop, "notes changed"):
            self.run_quiet(self.publisher())
        self.assertTrue(self.github.release["draft"])
        self.assertEqual(self.github.publishes, 0)

    def test_tampered_draft_asset_is_not_replaced_or_published(self):
        self.run_quiet(self.publisher(draft=True))
        self.github.release["assets"][0]["digest"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(auto.Stop, "differs"):
            self.run_quiet(self.publisher())
        self.assertEqual(self.github.uploads, 3)
        self.assertEqual(self.github.publishes, 0)

    def test_anonymous_download_corruption_is_detected_after_publication(self):
        self.github.tamper_public = True
        with self.assertRaisesRegex(auto.Stop, "download hash"):
            self.run_quiet(self.publisher())
        self.assertEqual(self.github.publishes, 1)
        self.github.tamper_public = False
        self.run_quiet(self.publisher())
        self.assertEqual(self.github.publishes, 1)

    def test_package_credentials_extra_zip_entry_and_wrong_installer_are_rejected(self):
        (self.package / "monitoring.json").write_text("{}")
        with self.assertRaisesRegex(auto.Stop, "Personal"):
            self.run_quiet(self.publisher())
        (self.package / "monitoring.json").unlink()
        self.run_quiet(self.publisher(draft=True))
        self.installer.write_bytes(b"changed")
        with self.assertRaisesRegex(auto.Stop, "Installer differs"):
            self.run_quiet(self.publisher())
        self.assertEqual(self.github.publishes, 0)

    def test_modified_package_and_wrong_installer_version_stop_before_signing_or_push(self):
        with patch.object(auto, "file_version", return_value=(1, 2, 3, 3)):
            with self.assertRaisesRegex(auto.Stop, "Installer pointer/version"):
                self.run_quiet(self.publisher())
        (self.package / "verification/x11-runtime.json").write_text('{"bundledX11TestPassed":true, "unexpected":"edit"}')
        with self.assertRaisesRegex(auto.Stop, "ZIP creation"):
            self.run_quiet(self.publisher())
        self.assertEqual(self.git.pushes, 0)
        self.assertEqual(self.github.creates, 0)

    def test_extra_entry_in_original_build_zip_is_rejected(self):
        original = Path((self.output / "LATEST-ZIP.txt").read_text(encoding="utf-8-sig"))
        with zipfile.ZipFile(original, "a") as archive:
            archive.writestr("fixture/extra.dll", b"unexpected")
        with self.assertRaisesRegex(auto.Stop, "different files"):
            self.run_quiet(self.publisher())
        self.assertEqual(self.git.pushes, 0)

    def test_source_changed_during_signing_is_not_pushed(self):
        real_prepare = auto.signing.prepare
        current = ["a" * 64]
        def prepare(*args, **kwargs):
            result = real_prepare(*args, **kwargs)
            current[0] = "c" * 64
            return result
        with patch.object(auto, "fingerprint", side_effect=lambda *args: current[0]), \
             patch.object(auto.signing, "prepare", side_effect=prepare):
            with self.assertRaisesRegex(auto.Stop, "Build inputs changed"):
                self.run_quiet(self.publisher())
        self.assertEqual(self.git.pushes, 0)
        self.assertEqual(self.github.creates, 0)

    def test_only_empty_incomplete_draft_upload_is_retried(self):
        self.run_quiet(self.publisher(draft=True))
        self.github.release["assets"][0].update(state="starter", size=0, digest=None)
        self.run_quiet(self.publisher())
        self.assertEqual(self.github.uploads, 4)
        self.assertEqual(self.github.creates, 1)
        self.assertEqual(self.github.publishes, 1)


class SourceAuditTests(unittest.TestCase):
    def test_draft_lookup_falls_back_to_authenticated_paginated_list(self):
        github = object.__new__(auto.GitHub)
        github.prefix = "/repos/example/AetherDesk"
        draft = {"id": 11, "tag_name": "v1.2.3-build4", "draft": True}
        def api(path, **kwargs):
            if "/releases/tags/" in path:
                return None
            self.assertIn("/releases?per_page=100&page=", path)
            return [draft]
        github.api = api
        self.assertEqual(github.find("v1.2.3-build4"), draft)

    def test_real_git_audit_blocks_secret_additions_and_preserves_main(self):
        with tempfile.TemporaryDirectory(prefix="aetherdesk-git-fixture-") as tmp:
            root = Path(tmp)
            def git(*args):
                subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)
            git("init", "-b", "main")
            git("config", "user.name", "Fixture publisher")
            git("config", "user.email", "publisher@example.invalid")
            git("remote", "add", "origin", "https://github.com/example/AetherDesk.git")
            (root / "readme.txt").write_text("base\n")
            git("add", "readme.txt")
            git("commit", "-m", "fixture")
            source = auto.Git(root)
            source.validate("example/AetherDesk")
            cases = ["-----BEGIN " + "PRIVATE KEY-----", "ghp_" + "a" * 40,
                     "10." + "23.45.67", "License " + "server status:"]
            for value in cases:
                (root / "new.txt").write_text(value)
                with self.assertRaises(auto.Stop):
                    source.audit()
            (root / "new.txt").write_text("fictional example\n")
            self.assertEqual(source.audit(), ["new.txt"])
            git("remote", "set-url", "origin", "https://example.invalid/unexpected.git")
            with self.assertRaises(auto.Stop):
                source.validate("example/AetherDesk")

    def test_authenticated_api_error_does_not_echo_response_body_or_token(self):
        github = object.__new__(auto.GitHub)
        github.prefix, github.token = "/repos/example/AetherDesk", "fixture-sensitive-value"
        failure = urllib.error.HTTPError("https://api.github.com/", 401, "fixture-sensitive-value", {}, io.BytesIO(b"fixture-sensitive-value"))
        with patch.object(auto.urllib.request, "build_opener") as opener:
            opener.return_value.open.side_effect = failure
            with self.assertRaises(auto.Stop) as result:
                github.api(github.prefix)
            self.assertNotIn("fixture-sensitive-value", str(result.exception))


if __name__ == "__main__":
    unittest.main(verbosity=2)
