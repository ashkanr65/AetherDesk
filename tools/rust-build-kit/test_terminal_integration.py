"""Exercise the production terminal setup against a real, isolated Bash PTY.

No SSH server, user history file, network connection or compiler is used.
"""
import os
import re
import select
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path


@unittest.skipUnless(sys.platform.startswith("linux"), "requires a Linux PTY")
class TerminalIntegration(unittest.TestCase):
    def test_prompt_and_arrow_history_keep_user_commands(self):
        import fcntl
        import pty
        import termios

        root = Path(__file__).resolve().parents[2]
        source = (root / "ui/rust/crates/ad-desktop/src/connection.rs").read_text(encoding="utf-8")
        match = re.search(r'const SHELL_INTEGRATION: &str = r#"(.*?)"#;', source, re.S)
        self.assertIsNotNone(match, "production shell integration must be available")
        script = match.group(1)
        for control in ["", "ignoredups", "ignorespace", "ignoreboth:erasedups"]:
            with self.subTest(control=control), tempfile.TemporaryDirectory(prefix="ad-terminal-") as folder:
                home = Path(folder)
                destination = home / "directory with spaces"
                destination.mkdir()
                history = home / "history"
                history.write_text("echo keep-existing-user-command\n", encoding="utf-8")
                master, slave = pty.openpty()
                attributes = termios.tcgetattr(slave)
                attributes[3] &= ~termios.ECHO
                termios.tcsetattr(slave, termios.TCSANOW, attributes)

                def controlling_terminal():
                    os.setsid()
                    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

                environment = os.environ.copy()
                environment.update(HOME=folder, HISTFILE=str(history), HISTSIZE="1000",
                                   HISTFILESIZE="1000", HISTCONTROL=control,
                                   HISTIGNORE="*__aetherdesk_emit_cwd*:ignore-fixture*",
                                   PS1="[PTY]$ ", PROMPT_COMMAND="", TERM="xterm-256color")
                child = subprocess.Popen(["bash", "--noprofile", "--norc", "-i"],
                                         stdin=slave, stdout=slave, stderr=slave,
                                         env=environment, preexec_fn=controlling_terminal,
                                         close_fds=True)
                os.close(slave)
                try:
                    def prompt():
                        output = bytearray()
                        deadline = time.monotonic() + 5
                        while time.monotonic() < deadline:
                            ready, _, _ = select.select([master], [], [], 0.1)
                            if ready:
                                output.extend(os.read(master, 65536))
                                if output.endswith(b"[PTY]$ "):
                                    return bytes(output)
                        self.fail(f"terminal did not produce a prompt: {bytes(output)!r}")

                    initial = prompt()
                    self.assertEqual(initial.count(b"[PTY]$ "), 1)
                    os.write(master, (script + "\n").encode())
                    setup = prompt()
                    self.assertIn(b"\r\x1b[2K", setup, "initial prompt must be erased before its replacement")
                    self.assertIn(b"\x1b]7;file://", setup, "folder reports remain enabled")
                    os.write(master, b"printf 'USER_COMMAND_OK\\n'\n")
                    self.assertIn(b"USER_COMMAND_OK", prompt())
                    quoted = "'" + str(destination).replace("'", "'\\''") + "'"
                    os.write(master, (" __aetherdesk_cd " + quoted + "\n").encode())
                    self.assertIn(str(destination).encode(), prompt())
                    # Production navigation uses framed output so the native
                    # client can hide only its command echo, not real errors.
                    token = "0123456789abcdef0123456789abcdef"
                    os.write(master, (" __aetherdesk_cd " + quoted + " '" + token + "'\n").encode())
                    framed = prompt()
                    self.assertIn(("\x1b]777;" + token + ";begin\x07").encode(), framed)
                    self.assertIn(("\x1b]777;" + token + ";end\x07").encode(), framed)
                    os.write(master, b"\x1b[A\n")
                    self.assertIn(b"USER_COMMAND_OK", prompt(), "Up must repeat the user command, not folder navigation")
                    os.write(master, b"printf '__CONTROL__%s\\n' \"$HISTCONTROL\"\n")
                    self.assertIn(("__CONTROL__" + control).encode(), prompt())
                    os.write(master, b"exit\n")
                    self.assertEqual(child.wait(timeout=5), 0)
                    stored = history.read_text(encoding="utf-8")
                    self.assertIn("echo keep-existing-user-command", stored)
                    self.assertIn("printf 'USER_COMMAND_OK", stored)
                    self.assertFalse(any(line.startswith(" __aetherdesk_cd ") for line in stored.splitlines()), stored)
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=5)
                    os.close(master)


if __name__ == "__main__":
    unittest.main()
