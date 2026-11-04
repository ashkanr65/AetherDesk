"""Isolated host-maintenance fixtures. Never use the real account's home."""
import importlib.util
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
sys.dont_write_bytecode = True

SOURCE = Path(os.environ.get("AETHERDESK_HOST_CLEANUP_SCRIPT") or (Path(__file__).resolve().parents[2] / "core/rust/crates/ad-ssh/src/host_cleanup.py"))
spec = importlib.util.spec_from_file_location("host_cleanup_fixture", str(SOURCE))
cleanup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cleanup)
LIVE_CACHE_VERSIONS = cleanup.live_cache_versions


class HostCleanupTests(unittest.TestCase):
    def setUp(self):
        cleanup.live_cache_versions = LIVE_CACHE_VERSIONS
        self.temp = tempfile.TemporaryDirectory(prefix="aetherdesk-cleanup-fixture-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.root = self.home / ".aetherdesk"
        self.root.mkdir(mode=0o700)
        self.connections = self.root / "connections"
        self.connections.mkdir(mode=0o700)

    def lease(self):
        identity = str(uuid.uuid4())
        job = subprocess.Popen([sys.executable, str(SOURCE), "lease", identity, "0.1.3"],
            env=dict(os.environ, HOME=str(self.home)), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        def stop():
            if job.poll() is None:
                job.terminate()
                try: job.wait(timeout=5)
                except subprocess.TimeoutExpired: job.kill(); job.wait()
            for stream in (job.stdin, job.stdout, job.stderr):
                if stream: stream.close()
        self.addCleanup(stop)
        self.assertTrue(select.select([job.stdout], [], [], 5)[0], "lease did not become ready")
        line = job.stdout.readline().decode().strip()
        self.assertEqual(line, "AETHERDESK_CLEANUP_READY_V1=" + str(self.connections / identity))
        return job, self.connections / identity

    def settle(self, predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            self.assertLess(time.monotonic(), deadline, "maintenance did not settle")
            time.sleep(0.01)

    def abandoned(self):
        path = self.connections / str(uuid.uuid4())
        path.mkdir(mode=0o700)
        (path / ".owned.json").write_text(json.dumps({"owner": cleanup.MAGIC, "id": path.name, "version": "0.1.3"}))
        (path / ".lease").touch()
        (path / "startup").mkdir()
        (path / "startup/environment.sh").write_text("private fixture")
        return path

    def test_eof_cleans_only_its_connection_and_preserves_another_tab_and_transfers(self):
        first, one = self.lease()
        second, two = self.lease()
        (one / "temporary").write_text("owned fixture")
        partial = self.root / "transfer.partial"
        partial.write_text("resumable fixture")
        user = self.home / "project.txt"
        user.write_text("saved work")
        first.stdin.close(); first.stdin = None
        self.assertEqual(first.wait(timeout=5), 0)
        self.assertFalse(one.exists())
        self.assertTrue(two.exists())
        self.assertIsNone(second.poll())
        self.assertEqual(partial.read_text(), "resumable fixture")
        self.assertEqual(user.read_text(), "saved work")

    def test_retry_cleans_unlocked_owned_residue_but_preserves_unknown_directories(self):
        stale = self.abandoned()
        unknown = self.connections / str(uuid.uuid4())
        unknown.mkdir(); (unknown / "user.txt").write_text("unknown")
        job, active = self.lease()
        self.settle(lambda: not stale.exists())
        self.assertTrue(unknown.exists())
        self.assertTrue(active.exists())
        self.assertIsNone(job.poll())

    def test_hup_cleans_the_owned_tree(self):
        job, path = self.lease()
        (path / "temporary").write_text("owned")
        job.send_signal(signal.SIGHUP)
        self.assertEqual(job.wait(timeout=5), 0)
        self.assertFalse(path.exists())

    def test_legacy_umask_directories_become_private_without_changing_user_files(self):
        self.root.chmod(0o775)
        self.connections.chmod(0o775)
        keep = self.root / "saved-user-data"
        keep.write_text("preserved")
        keep.chmod(0o640)
        self.lease()
        self.assertEqual(self.root.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.connections.stat().st_mode & 0o777, 0o700)
        self.assertEqual(keep.read_text(), "preserved")
        self.assertEqual(keep.stat().st_mode & 0o777, 0o640)

    def test_unsafe_root_reports_a_safe_reason_without_following_links(self):
        self.connections.rmdir()
        self.root.rmdir()
        outside = self.home / "outside"
        outside.mkdir(mode=0o755)
        self.root.symlink_to(outside)
        result = subprocess.run([sys.executable, str(SOURCE), "lease", str(uuid.uuid4()), "0.1.3"],
            env=dict(os.environ, HOME=str(self.home)), input=b"", stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr.strip(), b"AETHERDESK_CLEANUP_ERROR_V1=unsafe_state")
        self.assertEqual(outside.stat().st_mode & 0o777, 0o755)
        self.assertEqual(list(outside.iterdir()), [])

    def test_diagnostic_history_is_bounded_and_preserves_unknown_files(self):
        base = self.root / "diagnostics"; base.mkdir()
        for _ in range(23):
            (base / (str(uuid.uuid4()) + ".log")).write_bytes(b"AETHERDESK_DIAGNOSTIC_V1\nfixture")
        unknown = base / (str(uuid.uuid4()) + ".log")
        unknown.write_text("unmanaged user file")
        cleanup.prune_diagnostics(self.root)
        self.assertEqual(len(list(base.iterdir())), 21)
        self.assertTrue(unknown.exists())

    def test_connection_links_never_delete_files_outside_the_owned_tree(self):
        target = self.home / "outside"
        target.mkdir(); (target / "keep").write_text("saved data")
        stale = self.abandoned()
        (stale / "outside-link").symlink_to(target)
        linked = self.connections / str(uuid.uuid4())
        linked.symlink_to(target)
        self.lease()
        self.settle(lambda: not stale.exists())
        self.assertTrue(linked.is_symlink())
        self.assertEqual((target / "keep").read_text(), "saved data")

    def test_only_authoritatively_ended_desktops_are_swept(self):
        sessions = self.root / "sessions"
        sessions.mkdir()
        live = sessions / str(uuid.uuid4()); live.mkdir()
        (live / "application-work").write_text("retained")
        ended = sessions / str(uuid.uuid4()); ended.mkdir()
        (ended / "ended").write_text("AETHERDESK_SESSION_ENDED_V1 " + ended.name + "\n")
        self.lease()
        self.settle(lambda: not ended.exists())
        self.assertEqual((live / "application-work").read_text(), "retained")

    def cache(self, version):
        base = self.root / "agent"; base.mkdir(exist_ok=True)
        path = base / version; path.mkdir(); (path / "libs").mkdir()
        (path / ".aetherdesk-owned").write_bytes(cleanup.CACHE_MAGIC)
        (path / "runtime-manifest.json").write_text(json.dumps({"schema_version": 1, "files": [{"file_name": "fixture.so"}]}))
        (path / "libs/fixture.so").write_bytes(b"fixture")
        (path / "ad-agent").write_bytes(b"fixture")
        old = time.time() - 14 * 86400
        for file in cleanup.cache_files(path): os.utime(str(file), (old, old))
        return path

    def test_cache_retention_preserves_live_current_recent_and_unverified_files(self):
        live = self.cache("0.1.0")
        unused = self.cache("0.1.1")
        previous = self.cache("0.1.2")
        current = self.cache("0.1.3")
        cleanup.live_cache_versions = lambda _base, _versions: {"0.1.0"}
        cleanup.prune_caches(self.root, "0.1.3", set())
        self.assertTrue(live.exists()); self.assertFalse(unused.exists())
        self.assertTrue(previous.exists()); self.assertTrue(current.exists())
        pending = self.cache("0.0.9"); (pending / "upload.part").write_bytes(b"resume")
        unknown = self.cache("0.0.8"); (unknown / ".aetherdesk-owned").unlink()
        recent = self.cache("0.0.7"); (recent / "ad-agent").write_bytes(b"recent update")
        uploading = self.cache("0.0.6")
        cleanup.prune_caches(self.root, "0.1.3", {"0.0.6"})
        self.assertTrue(pending.exists()); self.assertTrue(unknown.exists()); self.assertTrue(recent.exists())
        self.assertTrue(uploading.exists())

    def test_hidden_processes_prevent_cache_pruning(self):
        old = self.cache("0.0.1")
        self.cache("0.1.2"); self.cache("0.1.3")
        cleanup.live_cache_versions = lambda _base, _versions: None
        cleanup.prune_caches(self.root, "0.1.3", set())
        self.assertTrue(old.exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
