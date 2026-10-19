"""Offline fixtures only: no application build, accounts or real signing key."""
import base64
import json
import os
from pathlib import Path
import tempfile
import unittest
import re
import subprocess
from unittest.mock import patch
import zipfile

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
import update_release as release


class UpdateReleaseTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "nt", "Production extractor uses Windows PowerShell")
    def test_production_extractor_accepts_inventory_and_rejects_traversal_and_extra_entries(self):
        source = (release.ROOT / "ui/rust/crates/ad-desktop/src/update_apply.rs").read_text()
        script = re.search(r'const EXTRACT: &str = r#"(.*?)"#;', source, re.S).group(1)
        with tempfile.TemporaryDirectory(prefix="aetherdesk-extract-fixture-") as tmp:
            root = Path(tmp)
            (root / "extract.ps1").write_text(script)
            (root / "inventory.json").write_text('[{"path":"AetherDesk.exe","bytes":4}]')
            powershell = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
            for index, entries in enumerate([["AetherDesk.exe"], ["../outside.txt"], ["AetherDesk.exe", "extra.dll"], ["AetherDesk.exe", "AetherDesk.exe"]]):
                archive = root / f"test{index}.zip"
                with zipfile.ZipFile(archive, "w") as stream:
                    for entry in entries:
                        stream.writestr(entry, b"test")
                destination = root / f"out{index}"
                destination.mkdir()
                result = subprocess.run([str(powershell), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                    "-File", str(root / "extract.ps1"), str(archive), str(destination), str(root / "inventory.json")],
                    capture_output=True, timeout=20)
                self.assertEqual(result.returncode == 0, index == 0)
                if index == 0:
                    self.assertEqual((destination / "AetherDesk.exe").read_bytes(), b"test")
                self.assertFalse((root / "outside.txt").exists())

    def test_signed_flat_package_matches_every_file_and_rejects_wrong_key(self):
        with tempfile.TemporaryDirectory(prefix="aetherdesk-update-test-") as tmp:
            root = Path(tmp)
            package = root / "package"
            (package / "verification").mkdir(parents=True)
            (package / "agent/linux-x86_64").mkdir(parents=True)
            (package / "AetherDesk.exe").write_bytes(b"test fixture only")
            os.utime(package / "AetherDesk.exe", (1, 1))
            (package / "release.json").write_text(json.dumps({"version": "1.2.3", "build": 4}))
            (package / "build-info.json").write_text(json.dumps({"version": "1.2.3", "build": 4, "runtime": "native-rust"}))
            (package / "verification/regression-catalog.json").write_text("{}")
            evidence = json.dumps({"compilerTestsPassed": True, "catalogSha256": release.sha(package / "verification/regression-catalog.json")})
            (package / "verification/windows-regressions.json").write_text(evidence)
            (package / "agent/linux-x86_64/regression-results.json").write_text(evidence)
            (package / "verification/x11-runtime.json").write_text('{"bundledX11TestPassed":true}')
            key = Ed25519PrivateKey.generate()
            config = root / "config.json"
            config.write_text(json.dumps({"publicKey": release.public(key)}))
            with patch.object(release, "load_key", return_value=key):
                manifest = release.prepare(package, root / "out", None, "fixture", config)
                with self.assertRaises(ValueError):
                    release.prepare(package, root / "out", None, "fixture", config)
            envelope = json.loads((root / "out/update.json").read_text())
            release.verify_release(root / "out", config)
            payload = base64.b64decode(envelope["manifest"])
            key.public_key().verify(base64.b64decode(envelope["signature"]), payload)
            with zipfile.ZipFile(root / "out" / manifest["portable"]["name"]) as archive:
                self.assertEqual(set(archive.namelist()), {f["path"] for f in manifest["files"]})
                self.assertIn("AetherDesk.exe", archive.namelist())
                self.assertEqual(archive.getinfo("AetherDesk.exe").date_time, (1980, 1, 1, 0, 0, 0))
                for file in manifest["files"]:
                    self.assertEqual(release.hashlib.sha256(archive.read(file["path"])).hexdigest(), file["sha256"])
            with patch.object(release, "load_key", return_value=Ed25519PrivateKey.generate()):
                with self.assertRaises(ValueError):
                    release.prepare(package, root / "wrong-key", None, "fixture", config)
            self.assertNotIn("PRIVATE", (root / "out/update.json").read_text())
            (root / "out" / manifest["portable"]["name"]).write_bytes(b"tampered")
            with self.assertRaises(ValueError):
                release.verify_release(root / "out", config)

    def test_paths_cannot_escape_or_use_windows_devices(self):
        for path in ["../outside", "C:/evil", "a\\b", "aux.txt", "a./b", "a//b", "a:b", "/absolute", "COM1.txt"]:
            self.assertFalse(release.safe_path(path), path)
        self.assertTrue(release.safe_path("runtime/GStreamer.dll"))


if __name__ == "__main__":
    unittest.main()
