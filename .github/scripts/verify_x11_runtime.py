"""Verify the private X11 payload without an installed X server or SDK on PATH."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import struct
import subprocess
import sys
import tempfile
import time


def receive_exact(stream, count):
    data = bytearray()
    while len(data) < count:
        chunk = stream.recv(count - len(data))
        if not chunk:
            raise RuntimeError("Bundled X server closed the connection unexpectedly")
        data.extend(chunk)
    return bytes(data)


def connect_display(port, cookie):
    stream = socket.create_connection(("127.0.0.1", port), timeout=1)
    try:
        protocol = b"MIT-MAGIC-COOKIE-1"
        setup = struct.pack("<BBHHHHH", ord("l"), 0, 11, 0, len(protocol), len(cookie), 0)
        stream.sendall(setup + protocol + b"\0" * (-len(protocol) % 4) + cookie)
        prefix = receive_exact(stream, 8)
        body = receive_exact(stream, struct.unpack_from("<H", prefix, 6)[0] * 4)
        return stream, prefix[0], body
    except BaseException:
        stream.close()
        raise


def create_cookie():
    # Match the client: VcXsrv's Windows text-mode reader treats 0x1a as EOF.
    cookie = bytearray()
    while len(cookie) < 16:
        value = secrets.randbelow(256)
        if value != 0x1a:
            cookie.append(value)
    return bytes(cookie)


def verify_x11_runtime(package):
    root = Path(package).resolve(strict=True) / "x11"
    required = [
        "vcxsrv.exe", "xauth.exe", "xkbcomp.exe", "libX11.dll", "libXau.dll",
        "libxcb.dll", "swrast_dri.dll", "vcruntime140.dll", "vcruntime140_1.dll",
        "msvcp140.dll", "fonts/misc/fonts.dir", "fonts/misc/fonts.alias",
        "xkbdata/rules/base", "locale/locale.alias", "licenses/COPYING",
        "licenses/README.txt", "licenses/vcxsrv-source.tar.gz", "bundle-info.json",
    ]
    for relative in required:
        if not (root / relative).is_file():
            raise RuntimeError("Missing bundled X11 file: " + relative)
    metadata = json.loads((root / "bundle-info.json").read_text(encoding="utf-8-sig"))
    digest = hashlib.sha256()
    with (root / "licenses/vcxsrv-source.tar.gz").open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    if digest.hexdigest() != metadata["sourceSha256"]:
        raise RuntimeError("Bundled X11 source archive checksum mismatch")

    with tempfile.TemporaryDirectory(prefix="aetherdesk-x11-check-") as temporary:
        scratch = Path(temporary)
        env = os.environ.copy()
        env["PATH"] = str(root) + os.pathsep + str(Path(os.environ["SystemRoot"]) / "System32")
        # Deliberately exclude the user's installed server, preferences and data.
        for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"]:
            env[name] = str(scratch)
        env["XKB_CONFIG_ROOT"] = str(root / "xkbdata")
        env["XLOCALEDIR"] = str(root / "locale")
        authority = scratch / "Xauthority"
        env["XAUTHORITY"] = str(authority)
        cookie = create_cookie()
        port = None
        for display in range(80, 120):
            try:
                with socket.socket() as probe:
                    probe.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
                    probe.bind(("0.0.0.0", 6000 + display))
                port = 6000 + display
                break
            except OSError:
                continue
        if port is None:
            raise RuntimeError("No free display for the bundled X11 check")
        for host in ["127.0.0.1", "localhost"]:
            subprocess.run(
                [str(root / "xauth.exe"), "-f", str(authority), "add",
                 f"{host}:{display}", ".", cookie.hex()],
                cwd=scratch, env=env, check=True, timeout=10, capture_output=True,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
        with (scratch / "process.log").open("wb") as output:
            process = subprocess.Popen(
                [str(root / "vcxsrv.exe"), f":{display}", "-multiwindow", "-notrayicon",
                 "-noreset", "-nowgl", "+iglx", "-auth", str(authority),
                 "-logfile", str(scratch / "server.log"), "-listen", "tcp", "-nolisten", "inet6"],
                cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=output,
                stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW,
            )
            try:
                deadline = time.monotonic() + 20
                while True:
                    if process.poll() is not None:
                        raise RuntimeError(f"Bundled X server exited during startup: {process.returncode}")
                    try:
                        stream, status, _ = connect_display(port, cookie)
                        break
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise RuntimeError("Bundled X server did not become ready")
                        time.sleep(0.1)
                with stream:
                    if status != 1:
                        raise RuntimeError("Bundled X server rejected its authentication cookie")
                    # EDA/Motif applications need the built-in core fonts, not
                    # just a server executable that happens to accept a socket.
                    stream.sendall(struct.pack("<BBHHH", 49, 0, 4, 8, 5) + b"fixed\0\0\0")
                    reply = receive_exact(stream, 32)
                    if reply[0] != 1 or struct.unpack_from("<H", reply, 8)[0] == 0:
                        raise RuntimeError("Bundled X server cannot find the fixed core font")
                    receive_exact(stream, struct.unpack_from("<I", reply, 4)[0] * 4)
                rejected, status, _ = connect_display(port, bytes([0x1a]) * 16)
                rejected.close()
                if status != 0:
                    raise RuntimeError("Bundled X server accepted an invalid authentication cookie")
                if process.poll() is not None:
                    raise RuntimeError("Bundled X server stopped after its first client disconnected")
            except BaseException:
                log = scratch / "server.log"
                if log.exists():
                    print(log.read_text(errors="replace")[-6000:])
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
    print("Bundled X11 verified: authenticated startup, rejected bad cookie, core fonts, exit cleanup.")


if __name__ == "__main__":
    verify_x11_runtime(sys.argv[1])
