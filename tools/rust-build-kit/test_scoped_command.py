"""Local Linux regression fixtures for the exact embedded SSH job supervisor.

No SSH host, application build or third-party Python dependency is used.
"""
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "core/rust/crates/ad-ssh/src/scoped_command.sh"


def running(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


class ScopedCommandTests(unittest.TestCase):
    def setUp(self):
        self.folder = tempfile.TemporaryDirectory(prefix="aether-job-test-")
        self.root = Path(self.folder.name)
        self.jobs = []
        self.other = subprocess.Popen(["sleep", "120"])

    def tearDown(self):
        try:
            for job in self.jobs:
                if job.poll() is None:
                    job.stdin.close()
                    try:
                        job.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        # Even a broken supervisor fixture must not leak its
                        # disposable children. Match only this private test dir.
                        for record in self.root.glob("aetherdesk-job.*/group"):
                            try:
                                pid = int(record.read_text().strip())
                                identity = Path(f"/proc/{pid}/cmdline").read_bytes()
                                if pid > 1 and str(record.parent).encode() in identity:
                                    os.killpg(pid, signal.SIGKILL)
                            except (OSError, ValueError):
                                pass
                        job.kill()
                        job.wait(timeout=3)
                job.stdout.close()
                job.stderr.close()
                if not job.stdin.closed:
                    job.stdin.close()
        finally:
            if self.other.poll() is None:
                self.other.terminate()
            self.other.wait(timeout=3)
            self.folder.cleanup()

    def start(self, command, release=True, extra_env=None):
        job = subprocess.Popen(["bash", str(SCRIPT), command], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               env={**os.environ, "TMPDIR": str(self.root), **(extra_env or {})})
        self.jobs.append(job)
        if release:
            job.stdin.write("AETHER_RUN\n")
            job.stdin.flush()
        return job

    def wait_pids(self, name="pids"):
        path = self.root / name
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if path.exists() and path.read_text().strip():
                return [int(p) for p in path.read_text().split()]
            time.sleep(0.02)
        self.fail("Job did not publish its process identities")

    def assert_clean(self, pids):
        deadline = time.monotonic() + 3
        while any(running(pid) for pid in pids) and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertFalse(any(running(pid) for pid in pids), "Owned descendants survived cleanup")
        self.assertIsNone(self.other.poll(), "An unrelated process was stopped")
        self.assertFalse(list(self.root.glob("aetherdesk-job.*")), "Private job files leaked")

    def descendants(self, ignore_term=False):
        prefix = "trap '' TERM; " if ignore_term else ""
        return prefix + f"sleep 120 & child=$!; printf '%s %s\\n' \"$$\" \"$child\" > '{self.root}/pids'; wait"

    def test_command_cannot_run_before_client_releases_gate(self):
        job = self.start(f"touch '{self.root}/ran'", release=False)
        time.sleep(0.15)
        self.assertFalse((self.root / "ran").exists())
        job.stdin.close()
        self.assertEqual(job.wait(timeout=3), 125)
        self.assert_clean([])

    def test_eof_cancels_descendants_and_preserves_unrelated_process(self):
        job = self.start(self.descendants())
        pids = self.wait_pids()
        job.stdin.close()
        self.assertEqual(job.wait(timeout=5), 125)
        self.assert_clean(pids)

    def test_term_ignoring_descendants_are_forcibly_stopped(self):
        job = self.start(self.descendants(ignore_term=True))
        pids = self.wait_pids()
        job.stdin.close()
        self.assertEqual(job.wait(timeout=5), 125)
        self.assert_clean(pids)

    def test_finished_command_cleans_background_children_and_preserves_status(self):
        job = self.start(f"sleep 120 & printf '%s\\n' \"$!\" > '{self.root}/pids'; printf out; printf err >&2; exit 7")
        pids = self.wait_pids()
        self.assertEqual(job.wait(timeout=5), 7)
        self.assertEqual(job.stdout.read(), "out")
        self.assertEqual(job.stderr.read(), "err")
        self.assert_clean(pids)

    def test_missing_heartbeats_expire_even_with_stdin_still_open(self):
        job = self.start(self.descendants())
        pids = self.wait_pids()
        self.assertEqual(job.wait(timeout=14), 124)
        self.assert_clean(pids)

    def test_heartbeats_keep_a_long_command_alive(self):
        job = self.start("sleep 11; exit 0")
        def renew():
            while job.poll() is None:
                try:
                    job.stdin.write("AETHER_PULSE\n")
                    job.stdin.flush()
                except (BrokenPipeError, ValueError):
                    break
                time.sleep(0.2)
        writer = threading.Thread(target=renew, daemon=True)
        writer.start()
        status = job.wait(timeout=16)
        self.assertEqual(status, 0, job.stderr.read() if status else "")
        writer.join(timeout=2)
        self.assert_clean([])

    def test_supervisor_signal_cleans_its_group(self):
        job = self.start(self.descendants())
        pids = self.wait_pids()
        job.send_signal(signal.SIGTERM)
        self.assertEqual(job.wait(timeout=5), 125)
        self.assert_clean(pids)

    def test_timeout_after_consuming_a_heartbeat_delimiter_preserves_the_lease(self):
        # Reproduce the timed-read boundary deterministically: Bash may consume
        # input before reporting timeout. A line read hides its consumed newline.
        # Non-consuming -t 0 probes are safe; consuming timed reads are faulted.
        shim = self.root / "read-boundary.sh"
        shim.write_text(r'''read() {
    local read_status argument inject_timeout=false polling=false
    for argument in "$@"; do
        case "$argument" in
            0) polling=true ;;
            0.*) inject_timeout=true ;;
        esac
    done
    builtin read "$@"
    read_status=$?
    if [[ "$inject_timeout" == true || "$polling" == true ]]; then
        printf x >> "$TMPDIR/read-boundary"
    fi
    if ((read_status == 0)) && [[ "$inject_timeout" == true ]]; then
        return 142
    fi
    return "$read_status"
}
''')
        job = self.start("sleep 2; exit 0", extra_env={"BASH_ENV": str(shim)})
        for _ in range(12):
            if job.poll() is not None:
                break
            try:
                job.stdin.write("AETHER_PULSE\n")
                job.stdin.flush()
            except BrokenPipeError:
                break
            time.sleep(0.05)
        status = job.wait(timeout=5)
        self.assertTrue((self.root / "read-boundary").exists(), "Read-boundary scenario was not exercised")
        self.assertEqual(status, 0, job.stderr.read() if status else "")
        self.assert_clean([])

    def test_malformed_or_unterminated_heartbeats_cancel_only_the_owned_group(self):
        for payload in ("AETHER_PULSEx\n", "AETHER_PULSE", "X" * 65, "\x00"):
            with self.subTest(payload=payload):
                path = self.root / "pids"
                if path.exists():
                    path.unlink()
                job = self.start(self.descendants())
                pids = self.wait_pids()
                job.stdin.write(payload)
                job.stdin.flush()
                if payload == "AETHER_PULSE":
                    job.stdin.close()
                self.assertEqual(job.wait(timeout=5), 125)
                self.assert_clean(pids)

    def test_heartbeat_split_across_read_timeouts_is_accepted(self):
        job = self.start("sleep 2; exit 0")
        for _ in range(3):
            job.stdin.write("AETHER_")
            job.stdin.flush()
            time.sleep(0.3)
            job.stdin.write("PULSE\n")
            job.stdin.flush()
            time.sleep(0.3)
        self.assertEqual(job.wait(timeout=5), 0)
        self.assert_clean([])


if __name__ == "__main__":
    unittest.main(verbosity=2)
