"""Release synchronization regression tests; all writes use temporary fixtures."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("release_version", Path(__file__).with_name("version.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.write("release.json", json.dumps({"version": "0.2", "build": 7}))
        self.write("core/rust/Cargo.toml", '[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n')
        self.write("core/rust/crates/ad-example/Cargo.toml", '[package]\nname = "ad-example"\nversion.workspace = true\n')
        self.write("core/rust/crates/independent/Cargo.toml", '[package]\nname = "independent"\nversion = "1.0.0"\n')
        self.write("ui/flutter/ad_remote_desktop/pubspec.yaml", "name: aetherdesk\nversion: 0.1.0+1\n# Café survives Windows encoding.\n")
        self.write("core/rust/Cargo.lock", '''version = 4
[[package]]
name = "ad-example"
version = "0.1.0"
dependencies = [
 "ad-example 0.1.0",
 "ad-example 0.1.0 (registry+https://example.test)",
]
[[package]]
name = "ad-example"
version = "0.1.0"
source = "registry+https://example.test"
[[package]]
name = "independent"
version = "1.0.0"
''')

    def write(self, name, contents):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents, encoding="utf-8")

    def read(self, name):
        return (self.root / name).read_text(encoding="utf-8")

    def test_two_component_release_and_lock_dependency_references(self):
        self.assertEqual(module.sync(self.root), {"version": "0.2", "normalized": "0.2.0", "build": 7})
        self.assertIn('version = "0.2.0"', self.read("core/rust/Cargo.toml"))
        self.assertIn('"ad-example 0.2.0"', self.read("core/rust/Cargo.lock"))
        self.assertIn('"ad-example 0.1.0 (registry+https://example.test)"', self.read("core/rust/Cargo.lock"))
        self.assertIn('name = "ad-example"\nversion = "0.1.0"\nsource', self.read("core/rust/Cargo.lock"))
        self.assertIn('name = "independent"\nversion = "1.0.0"', self.read("core/rust/Cargo.lock"))
        self.assertIn("version: 0.2.0+7", self.read("ui/flutter/ad_remote_desktop/pubspec.yaml"))
        self.assertIn("Café", self.read("ui/flutter/ad_remote_desktop/pubspec.yaml"))
        self.assertIn("const appVersion = '0.2';", self.read("ui/flutter/ad_remote_desktop/lib/app_version.dart"))
        module.sync(self.root, check=True)

    def test_three_components(self):
        self.write("release.json", '{"version":"0.2.3","build":65535}')
        self.assertEqual(module.sync(self.root)["normalized"], "0.2.3")
        self.assertIn("version: 0.2.3+65535", self.read("ui/flutter/ad_remote_desktop/pubspec.yaml"))

    def test_check_never_changes_stale_manifests(self):
        before = self.read("core/rust/Cargo.toml")
        with self.assertRaisesRegex(ValueError, "need synchronization"):
            module.sync(self.root, check=True)
        self.assertEqual(self.read("core/rust/Cargo.toml"), before)
        self.assertFalse((self.root / "ui/flutter/ad_remote_desktop/lib/app_version.dart").exists())

    def test_invalid_versions_and_builds(self):
        for version in ["0", "01.2", "0.2.3.4", "1.0-beta", "65536.0", "-1.2", "１.２", 0.2, None]:
            with self.subTest(version=version):
                self.write("release.json", json.dumps({"version": version, "build": 1}))
                with self.assertRaises(ValueError):
                    module.sync(self.root)
        for build in [0, -1, 65536, True, "1", 1.5, None]:
            with self.subTest(build=build):
                self.write("release.json", json.dumps({"version": "0.2", "build": build}))
                with self.assertRaises(ValueError):
                    module.sync(self.root)

    def test_missing_version_stops_before_any_write(self):
        self.write("ui/flutter/ad_remote_desktop/pubspec.yaml", "name: aetherdesk\n")
        with self.assertRaisesRegex(ValueError, "Flutter version"):
            module.sync(self.root)
        self.assertIn('version = "0.1.0"', self.read("core/rust/Cargo.toml"))

    def test_missing_lockfile_is_an_error(self):
        (self.root / "core/rust/Cargo.lock").unlink()
        with self.assertRaisesRegex(ValueError, "Cargo.lock is required"):
            module.sync(self.root)


if __name__ == "__main__":
    unittest.main()
