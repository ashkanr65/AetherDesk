"""Execute the production collector against disposable proc/utility fixtures.

No SSH, real server commands, application compilation or licence queries.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('monitor_host.sh').resolve()
BASH = shutil.which('bash') or next((str(p) for p in (
    Path('C:/Program Files/Git/bin/bash.exe'),
    Path('C:/Program Files/Git/usr/bin/bash.exe')) if p.is_file()), None)


@unittest.skipUnless(BASH, 'Bash is required for the Linux collector fixture')
class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.proc = self.root / 'proc'
        (self.proc / 'net').mkdir(parents=True)
        (self.root / 'bin').mkdir()
        (self.root / 'homes' / 'AB').mkdir(parents=True)
        (self.root / 'homes' / 'CD space').mkdir()
        self.write('stat', 'cpu  100 20 30 400 50 60 70 80 900 1000\n')
        self.write('meminfo', 'MemTotal: 1000 kB\nMemAvailable: 600 kB\nMemFree: 10 kB\nBuffers: 20 kB\nCached: 30 kB\nSwapTotal: 50 kB\nSwapFree: 40 kB\n')
        self.write('uptime', '90061.50 200.00\n')
        self.write('net/dev', 'Inter-|Receive|Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets\n lo: 99 1 0 0 0 0 0 0 88 1 0 0 0 0 0 0\n eth0: 1234 1 0 0 0 0 0 0 5678 1 0 0 0 0 0 0\n')
        self.write('who', 'user1 pts/0 2025-01-02 09:00 (host)\nuser1 pts/1 2025-01-02 09:01\n')
        self.write('df', 'Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda 100 50 45 53% /space dir\n')
        self.utility('du', 'printf du >> "$AETHER_MONITOR_PROC_ROOT/calls"; printf "128\\t%s/AB\\n" "$AETHER_MONITOR_HOME_ROOT"; exit 1')
        self.utility('timeout', '[ \"$1\" = 8 ] || exit 99; shift; exec \"$@\"')
        self.utility('stat', 'printf owner')
        self.utility('uname', 'printf "fixture-host\\n"')
        self.utility('who', 'printf who >> "$AETHER_MONITOR_PROC_ROOT/calls"; cat "$AETHER_MONITOR_PROC_ROOT/who"')
        self.utility('df', '[ \"$1\" = -Pk ] && [ \"$2\" = -- ] && [ \"$3\" = \"$HOME\" ] || exit 99; printf df >> "$AETHER_MONITOR_PROC_ROOT/calls"; cat "$AETHER_MONITOR_PROC_ROOT/df"')

    def write(self, name, text):
        (self.proc / name).write_bytes(text.encode('utf-8'))

    def utility(self, name, text):
        path = self.root / 'bin' / name
        path.write_bytes(('#!/bin/sh\n' + text + '\n').encode('utf-8'))
        path.chmod(0o755)

    def run_mode(self, mode):
        # Resolve fixture paths inside Bash, also supporting Git Bash on Windows.
        wrapper = 'fixture=$(cd "$1" && pwd); export PATH="$fixture/bin:$PATH"; export AETHER_MONITOR_PROC_ROOT="$fixture/proc"; export AETHER_MONITOR_HOME_ROOT="$fixture/homes"; export HOME="$fixture/homes/AB"; exec sh "$2" "$3"'
        return subprocess.run([BASH, '-c', wrapper, 'monitor-fixture', self.root.as_posix(), SCRIPT.as_posix(), mode],
                              text=True, capture_output=True, timeout=10, env=dict(os.environ))

    def test_fast_counters_exclude_guest_double_count_and_do_not_query_users_or_disks(self):
        result = self.run_mode('fast')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('HOST\tfixture-host\n', result.stdout)
        self.assertIn('CPU\t810\t450\n', result.stdout)
        self.assertIn('MEM\t1000\t600\t50\t40\n', result.stdout)
        self.assertIn('NET\teth0\t1234\t5678\n', result.stdout)
        self.assertNotIn('NET\tlo', result.stdout)
        self.assertFalse((self.proc / 'calls').exists())

    def test_slow_preserves_sessions_and_paths_and_marks_unavailable_queries(self):
        result = self.run_mode('slow')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('user1 pts/0', result.stdout)
        self.assertIn('user1 pts/1', result.stdout)
        self.assertIn('/space dir', result.stdout)
        self.assertIn('HOME\t', result.stdout)
        self.assertNotIn('du', (self.proc / 'calls').read_text())
        self.utility('who', 'exit 1')
        self.utility('df', 'exit 1')
        result = self.run_mode('slow')
        self.assertEqual(result.returncode, 0)
        self.assertIn('ERR\tusers\t', result.stdout)
        self.assertIn('ERR\tdisks\t', result.stdout)
        self.assertEqual(self.run_mode('unknown').returncode, 2)

    def test_home_scan_is_explicit_bounded_and_preserves_partial_and_unknown_sizes(self):
        result = self.run_mode('homes')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('HOME_DIR\towner\t', result.stdout)
        self.assertIn('/CD space', result.stdout)
        self.assertIn('HOME_SIZE\t128\t', result.stdout)
        self.assertIn('ERR\thomes\t', result.stdout)
        self.assertNotIn('HOME_SIZE\t0\t', result.stdout)
        self.assertEqual((self.proc / 'calls').read_text(), 'du')
        # The periodic paths never invoke du, even when its fixture fails.
        (self.proc / 'calls').unlink()
        self.run_mode('slow')
        self.run_mode('fast')
        self.assertNotIn('du', (self.proc / 'calls').read_text())


if __name__ == '__main__':
    unittest.main(verbosity=2)
