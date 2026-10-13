"""Packaging checks use temporary fake assets; no app launch or runtime install."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("verify_editor", Path(__file__).with_name("Verify-Editor.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EditorPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.names = ["index.html", "editor.js", "editor.css", "editor.worker.js", "json.worker.js", "css.worker.js", "html.worker.js", "ts.worker.js", "codicon.ttf"]
        for name in self.names:
            (self.root / name).write_text("fixture", encoding="utf-8")
        self.manifest()

    def manifest(self):
        (self.root / "manifest.json").write_text(json.dumps({"version": 1, "files": self.names}), encoding="utf-8")

    def test_complete_offline_assets(self):
        self.assertEqual(module.verify_assets(self.root), 9)

    def test_missing_worker(self):
        (self.root / "ts.worker.js").unlink()
        with self.assertRaisesRegex(ValueError, "Missing or empty"):
            module.verify_assets(self.root)

    def test_manifest_path_escape_and_duplicates(self):
        for name in ["../outside", "..\\outside", "editor.js"]:
            with self.subTest(name=name):
                self.names.append(name)
                self.manifest()
                with self.assertRaises(ValueError):
                    module.verify_assets(self.root)
                self.names.pop()

    def test_missing_font(self):
        self.names.remove("codicon.ttf")
        self.manifest()
        with self.assertRaisesRegex(ValueError, "font"):
            module.verify_assets(self.root)


if __name__ == "__main__":
    unittest.main()
