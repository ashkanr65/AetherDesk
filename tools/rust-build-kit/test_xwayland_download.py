"""Exercise the production source downloader offline; no build or network use."""
import hashlib
import io
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import unittest


SCRIPT = Path(os.environ.get("AETHERDESK_XWAYLAND_SCRIPT", str(
    Path(__file__).resolve().parents[1] / "build-xwayland.sh")))
BASH = os.environ.get("AETHERDESK_TEST_BASH", "bash")
STUBS = r'''
curl_calls=0
curl() {
  printf '%s\n' "$*" >> "$CALL_LOG"
  curl_calls=$((curl_calls + 1))
  local url='' output=''
  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      --output) output="$2"; shift 2 ;;
      https://*) url="$1"; shift ;;
      *) shift ;;
    esac
  done
  # Each retry must begin with a clean partial file.
  [[ ! -e "$output" ]] || return 99
  case "$SCENARIO" in
    tls-fallback)
      if [[ "$url" == *primary* ]]; then printf 'partial' > "$output"; return 35; fi ;;
    tls-retry)
      if [[ "$curl_calls" -lt 3 ]]; then printf 'partial' > "$output"; return 35; fi ;;
    corrupt-fallback)
      if [[ "$url" == *primary* ]]; then printf 'wrong archive' > "$output"; return 0; fi ;;
    all-fail) printf 'partial' > "$output"; return 35 ;;
    all-corrupt) printf 'wrong archive' > "$output"; return 0 ;;
  esac
  cp "$GOOD_ARCHIVE" "$output"
}
sleep() { printf '%s\n' "$*" >> "$SLEEP_LOG"; }
tar() {
  printf '%s\n' "$*" >> "$TAR_LOG"
  command tar "$@"
}
'''


class DownloadTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix="aether-xwayland-fetch-")
        self.addCleanup(self.folder.cleanup)
        self.root = Path(self.folder.name)
        self.work = self.root / "work"
        self.work.mkdir()
        self.good = self.root / "good.tar.xz"
        data = b"verified source fixture\n"
        with tarfile.open(self.good, "w:xz") as archive:
            entry = tarfile.TarInfo("source-fixture/README")
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
        self.checksum = hashlib.sha256(self.good.read_bytes()).hexdigest()
        self.env = {**os.environ, "GOOD_ARCHIVE": self.good.as_posix(),
                    "CALL_LOG": (self.root / "curl.log").as_posix(),
                    "TAR_LOG": (self.root / "tar.log").as_posix(),
                    "SLEEP_LOG": (self.root / "sleep.log").as_posix()}

    def run_fetch(self, scenario, urls=None, name="source-fixture", checksum=None):
        self.env["SCENARIO"] = scenario
        urls = urls or ["https://primary.test/source", "https://mirror.test/source"]
        command = 'source ' + shlex.quote(SCRIPT.as_posix()) + '\n' + STUBS
        command += '\nfetch ' + ' '.join(shlex.quote(value) for value in
                                          [name, checksum or self.checksum, *urls])
        return subprocess.run([BASH, "-c", command], cwd=self.work, env=self.env,
                              capture_output=True, text=True, timeout=10)

    def calls(self):
        log = self.root / "curl.log"
        return log.read_text().splitlines() if log.exists() else []

    def assert_verified(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.work / "source-fixture.tar.xz").read_bytes(), self.good.read_bytes())
        self.assertEqual((self.work / "source-fixture/README").read_text(), "verified source fixture\n")
        self.assertFalse((self.work / "source-fixture.tar.xz.part").exists())

    def test_ssl_failure_uses_mirror_and_discards_partial_download(self):
        self.assert_verified(self.run_fetch("tls-fallback"))
        calls = self.calls()
        self.assertEqual(len(calls), 2)
        self.assertIn("https://primary.test/source", calls[0])
        self.assertIn("https://mirror.test/source", calls[1])
        for call in calls:
            self.assertIn("--proto =https --proto-redir =https", call)
            self.assertIn("--connect-timeout 15 --max-time 240", call)
            self.assertNotIn("--insecure", call)
            self.assertNotIn("--retry-all-errors", call)

    def test_ssl_failures_are_retried_on_older_curl(self):
        self.assert_verified(self.run_fetch("tls-retry", ["https://primary.test/source"]))
        self.assertEqual(len(self.calls()), 3)

    def test_bad_checksum_uses_mirror_before_extracting(self):
        self.assert_verified(self.run_fetch("corrupt-fallback"))
        self.assertEqual(len(self.calls()), 2)
        self.assertEqual(len((self.root / "tar.log").read_text().splitlines()), 1)

    def test_all_ssl_failures_stop_without_extraction_or_partial_files(self):
        result = self.run_fetch("all-fail")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(self.calls()), 6)
        self.assertIn("every HTTPS source three times", result.stderr)
        self.assertFalse((self.root / "tar.log").exists())
        self.assertEqual(list(self.work.iterdir()), [])

    def test_all_bad_checksums_stop_without_extracting(self):
        result = self.run_fetch("all-corrupt")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(self.calls()), 6)
        self.assertFalse((self.root / "tar.log").exists())
        self.assertEqual(list(self.work.iterdir()), [])

    def test_valid_cached_archive_needs_no_network(self):
        shutil.copyfile(self.good, self.work / "source-fixture.tar.xz")
        self.assert_verified(self.run_fetch("all-fail"))
        self.assertEqual(self.calls(), [])

    def test_corrupt_cached_archive_is_replaced(self):
        (self.work / "source-fixture.tar.xz").write_bytes(b"broken cached file")
        (self.work / "source-fixture.tar.xz.part").write_bytes(b"old partial file")
        self.assert_verified(self.run_fetch("tls-fallback"))

    def test_unpinned_or_unsafe_requests_are_rejected_before_network(self):
        for kwargs in ({"urls": ["http://primary.test/source"]},
                       {"name": "../source"}, {"checksum": "invalid"}):
            with self.subTest(kwargs=kwargs):
                self.assertNotEqual(self.run_fetch("success", **kwargs).returncode, 0)
                self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()
