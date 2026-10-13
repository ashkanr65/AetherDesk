"""Offline regression checks for the self-contained assistant package."""
import copy
import importlib.util
import io
import json
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import assistant_bundle as bundle

spec = importlib.util.spec_from_file_location('verify_assistant', Path(__file__).with_name('Verify-Assistant.py'))
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


class AssistantBundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='aetherdesk-assistant-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cache = self.root / 'cache'
        self.cache.mkdir()
        self.destination = self.root / 'package with spaces'
        self.pin = copy.deepcopy(bundle.read_pin())
        self.pin['wheels'] = [next(x for x in self.pin['wheels'] if x['name'] == 'langgraph')]
        self.archive = self.cache / self.pin['python']['filename']
        with zipfile.ZipFile(self.archive, 'w') as archive:
            archive.writestr('python.exe', b'MZfixture')
            archive.writestr('python313.zip', b'fixture standard library')
            archive.writestr('python313._pth', 'original configuration')
            archive.writestr('LICENSE.txt', 'Python license fixture')
        self.pin['python']['sha256'] = bundle.sha256(self.archive)
        self.wheel = self.cache / self.pin['wheels'][0]['filename']
        with zipfile.ZipFile(self.wheel, 'w') as archive:
            archive.writestr('langgraph/graph/__init__.py', '# fixture')
            archive.writestr('langgraph.dist-info/LICENSE', 'MIT license fixture')
        self.pin['wheels'][0]['sha256'] = bundle.sha256(self.wheel)
        for license in self.pin['licenses']:
            file = self.cache / license['filename']
            file.write_text('MIT license fixture', encoding='utf-8')
            license['sha256'] = bundle.sha256(file)
        self.server = self.root / 'server.py'
        self.server.write_text('# fixture server', encoding='utf-8')

    def build(self):
        return bundle.bundle(self.destination, self.cache, self.pin, self.server)

    def verify(self):
        with patch.object(verify, 'read_pin', return_value=self.pin):
            return verify.verify_files(self.destination)

    def test_complete_package_and_isolated_import_paths(self):
        runtime = self.build()
        executable, metadata = self.verify()
        self.assertEqual(executable, runtime / 'python' / 'python.exe')
        self.assertEqual(metadata['engine'], 'langgraph')
        self.assertNotIn('import site', (runtime / 'python' / 'python313._pth').read_text())
        self.assertTrue((runtime / 'packages' / 'langgraph.dist-info' / 'LICENSE').is_file())

    def test_corrupt_cached_python_and_wheel_rejected(self):
        for file in [self.archive, self.wheel]:
            original = file.read_bytes()
            file.write_bytes(b'corrupt')
            with self.assertRaisesRegex(ValueError, 'Cached assistant artifact failed'):
                self.build()
            self.assertFalse((self.destination / 'ai' / 'assistant').exists())
            file.write_bytes(original)

    def test_corrupt_cached_license_rejected(self):
        (self.cache / self.pin['licenses'][0]['filename']).write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'Cached assistant artifact failed'):
            self.build()

    def test_download_failure_removes_partial_files(self):
        self.wheel.unlink()
        response = io.BytesIO(b'corrupt downloaded wheel')
        response.url = self.pin['wheels'][0]['url']
        with patch.object(bundle.urllib.request, 'urlopen', return_value=response):
            with self.assertRaisesRegex(ValueError, 'Downloaded assistant artifact failed'):
                self.build()
        self.assertFalse(self.wheel.exists())
        self.assertEqual(list(self.cache.glob('*.partial')), [])

    def test_unsafe_archive_paths_duplicates_and_links_rejected(self):
        self.assertFalse(bundle.safe_relative('bad\\path'))
        for names in [('.. /escape',), ('../escape',), ('/absolute',), ('C:/escape',),
                      ('NUL.txt',), ('name.',), ('same', 'SAME')]:
            with self.subTest(names=names):
                with zipfile.ZipFile(self.wheel, 'w') as archive:
                    for name in names:
                        archive.writestr(name, 'payload')
                self.pin['wheels'][0]['sha256'] = bundle.sha256(self.wheel)
                with self.assertRaisesRegex(ValueError, 'Unsafe assistant archive entry'):
                    self.build()
                self.assertFalse((self.destination / 'ai' / 'assistant').exists())
        with zipfile.ZipFile(self.wheel, 'w') as archive:
            entry = zipfile.ZipInfo('link')
            entry.create_system = 3
            entry.external_attr = (stat.S_IFLNK | 0o777) << 16
            archive.writestr(entry, '../escape')
        self.pin['wheels'][0]['sha256'] = bundle.sha256(self.wheel)
        with self.assertRaisesRegex(ValueError, 'Unsafe assistant archive entry'):
            self.build()

    def test_packages_cannot_overwrite_each_other(self):
        output = self.root / 'unpacked'
        bundle.extract_archive(self.wheel, output, wheel=True)
        with self.assertRaisesRegex(ValueError, 'overwrite another package'):
            bundle.extract_archive(self.wheel, output, wheel=True)

    def test_tampering_additions_and_missing_files_rejected(self):
        runtime = self.build()
        server = runtime / 'server.py'
        original = server.read_bytes()
        server.write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError, 'integrity verification'):
            self.verify()
        server.write_bytes(original)
        extra = runtime / 'unexpected.py'
        extra.write_text('extra')
        with self.assertRaisesRegex(ValueError, 'every packaged file'):
            self.verify()
        extra.unlink()
        server.unlink()
        with self.assertRaisesRegex(ValueError, 'every packaged file'):
            self.verify()

    def test_existing_bundle_never_overlaid(self):
        self.build()
        with self.assertRaisesRegex(ValueError, 'Refusing to overlay'):
            self.build()
        self.verify()

    def test_manifest_rejects_different_dependency_lock(self):
        runtime = self.build()
        (runtime / 'runtime-lock.json').write_text('{}', encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'dependency lock'):
            self.verify()

    def test_pins_require_official_https_hashes_and_langgraph(self):
        for field, value in [('executable', '../python.exe'), ('engine', 'other')]:
            invalid = copy.deepcopy(self.pin)
            invalid[field] = value
            file = self.root / 'invalid.json'
            file.write_text(json.dumps(invalid), encoding='utf-8')
            with self.assertRaises(ValueError):
                bundle.read_pin(file)
        invalid = copy.deepcopy(self.pin)
        invalid['wheels'][0]['url'] = 'https://example.org/' + invalid['wheels'][0]['filename']
        file.write_text(json.dumps(invalid), encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'official PyPI'):
            bundle.read_pin(file)


if __name__ == '__main__':
    unittest.main()
