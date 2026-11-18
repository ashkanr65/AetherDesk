"""Linux integration checks for read-only planning and non-disruptive staging."""
import concurrent.futures
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest import mock


@unittest.skipUnless(os.name == "posix", "Linux agent staging")
class UpgradeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        source = Path(__file__).resolve().parents[1] / "agent-upgrade.py"
        spec = importlib.util.spec_from_file_location("agent_upgrade", str(source))
        cls.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.module)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.home.mkdir(mode=0o700)

    def tearDown(self):
        self.temp.cleanup()

    def bundle(self, name, executable="/usr/bin/python3", library=b"old runtime"):
        root = self.root / name
        (root / "libs").mkdir(parents=True)
        (root / "licenses").mkdir()
        (root / "licenses/NOTICE").write_text("Test fixture licence notice")
        (root / "version.txt").write_text("1.2.3\n")
        shutil.copy2(executable, str(root / "ad-agent"))
        (root / "libs/example.so").write_bytes(library)
        manifest = {"schema_version": 1, "files": [{"file_name": "example.so", "size": len(library), "sha256": hashlib.sha256(library).hexdigest()}]}
        (root / "runtime-manifest.json").write_text(json.dumps(manifest))
        return self.module.inspect_bundle(root)

    def test_dry_plan_has_no_writes_and_running_old_agent_survives_same_version_staging(self):
        old = self.bundle("old")
        new = self.bundle("new", "/bin/sh", b"new runtime")
        self.module.destination(self.home, new)
        self.assertEqual(list(self.home.iterdir()), [])
        previous = Path(self.module.stage_bundle(self.home, old)["path"])
        process = subprocess.Popen([str(previous / "ad-agent"), "-c", "import time; time.sleep(60)"])
        try:
            time.sleep(0.1)
            self.assertIsNone(process.poll())
            upgraded = Path(self.module.stage_bundle(self.home, new)["path"])
            self.assertNotEqual(previous, upgraded)
            self.assertIsNone(process.poll())
            self.assertEqual(self.module.digest(previous / "ad-agent"), old["agent_sha256"])
            self.assertEqual((previous / "libs/example.so").read_bytes(), b"old runtime")
            self.assertEqual((upgraded / "libs/example.so").read_bytes(), b"new runtime")
            self.assertTrue((upgraded / "licenses/NOTICE").is_file())
        finally:
            process.terminate()
            process.wait(timeout=5)

    def test_published_bundle_is_not_repaired_or_overwritten(self):
        bundle = self.bundle("source")
        target = Path(self.module.stage_bundle(self.home, bundle)["path"])
        (target / "libs/example.so").chmod(0o600)
        (target / "libs/example.so").write_bytes(b"changed")
        with self.assertRaises(ValueError):
            self.module.stage_bundle(self.home, bundle)
        self.assertEqual((target / "libs/example.so").read_bytes(), b"changed")

    def test_source_and_destination_links_are_rejected_without_following_them(self):
        bundle = self.bundle("source")
        outside = self.root / "outside"
        outside.mkdir()
        (outside / "keep").write_text("untouched")
        (self.home / ".aetherdesk").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.module.stage_bundle(self.home, bundle)
        self.assertEqual([p.name for p in outside.iterdir()], ["keep"])
        library = bundle["root"] / "libs/example.so"
        library.unlink()
        library.symlink_to(outside / "keep")
        with self.assertRaises(ValueError):
            self.module.inspect_bundle(bundle["root"])

    def test_failed_copy_cleans_only_its_own_marked_staging_directory(self):
        bundle = self.bundle("source")
        parent = self.module.destination(self.home, bundle, create=True).parent
        other = parent / ".stage-another-task"
        other.mkdir()
        (other / "keep").write_text("untouched")
        with mock.patch.object(self.module.shutil, "copyfileobj", side_effect=OSError("fixture interruption")):
            with self.assertRaises(OSError):
                self.module.stage_bundle(self.home, bundle)
        self.assertEqual(sorted(p.name for p in parent.iterdir()), [other.name])
        self.assertEqual((other / "keep").read_text(), "untouched")

    def test_cleanup_rejects_a_mounted_staging_root(self):
        work = self.home / ".stage-test"
        work.mkdir()
        (work / ".stage-owner.json").write_text(json.dumps({"project": "aetherdesk", "stage": "test", "uid": os.geteuid()}))
        (work / "keep").write_text("owned but mounted")
        with mock.patch.object(self.module.os.path, "ismount", return_value=True):
            with self.assertRaises(ValueError):
                self.module.cleanup_stage(work, self.home, "test")
        self.assertEqual((work / "keep").read_text(), "owned but mounted")

    def test_concurrent_stagers_publish_once_without_replacing_the_winner(self):
        bundle = self.bundle("source")
        self.module.destination(self.home, bundle, create=True)
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda _: self.module.stage_bundle(self.home, bundle), range(2)))
        self.assertEqual(sum(r["staged"] for r in results), 1)
        target = Path(results[0]["path"])
        self.assertEqual(self.module.digest(target / "ad-agent"), bundle["agent_sha256"])
        self.assertFalse(list(target.parent.glob(".stage-*")))

    def test_identity_vector_matches_rust_and_ignores_manifest_order(self):
        files = [{"file_name": "a.so", "size": 3, "sha256": "02" * 32}, {"file_name": "b.so", "size": 4, "sha256": "03" * 32}]
        identity = self.module.bundle_identity("01" * 32, {"files": files})
        self.assertEqual(identity, self.module.bundle_identity("01" * 32, {"files": list(reversed(files))}))
        self.assertEqual(identity, "984f5d22efb8afaf3aaaa72211bd6ce6bad7b24feb59446d052d06b5d59286d1")


if __name__ == "__main__":
    unittest.main()
