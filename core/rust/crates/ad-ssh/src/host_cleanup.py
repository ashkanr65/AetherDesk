"""Private AetherDesk connection leases. No user files or transfer stages.

An SSH channel holds stdin and the lease lock open. EOF/HUP removes its private
temporary tree. Subsequent connections retry abandoned, unlocked trees. Shared
agent caches require an explicit ownership marker, an age threshold, a verified
manifest layout and no live process/connection references before removal.
"""
import fcntl
import errno
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import sys
import time

MAGIC = "AETHERDESK_CONNECTION_V1"
CACHE_MAGIC = b"AETHERDESK_AGENT_CACHE_V1\n"
UUID = re.compile(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\Z")
VERSION = re.compile(r"[0-9]+(?:\.[0-9]+){0,2}\Z")


def owned(path, directory=False):
    meta = path.lstat()
    if meta.st_uid != os.geteuid() or stat.S_ISLNK(meta.st_mode):
        raise ValueError("Unowned or linked AetherDesk path")
    if not (stat.S_ISDIR(meta.st_mode) if directory else stat.S_ISREG(meta.st_mode)):
        raise ValueError("Unexpected AetherDesk file type")
    return meta


def private_dir(path):
    try:
        path.mkdir(mode=0o700)
    except FileExistsError:
        pass
    owned(path, True)
    # Older releases created this account's AetherDesk root under the shell's
    # umask (often 002). Repair only an owned, real directory, through a checked
    # descriptor; never chmod a link or relax another account's permissions.
    fd = os.open(str(path), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        meta = os.fstat(fd)
        if meta.st_uid != os.geteuid() or not stat.S_ISDIR(meta.st_mode):
            raise ValueError("Unowned AetherDesk directory")
        if stat.S_IMODE(meta.st_mode) != 0o700:
            os.fchmod(fd, 0o700)
    finally:
        os.close(fd)
    return path


def small(path, limit=4096):
    if owned(path).st_size > limit:
        raise ValueError("Oversized AetherDesk metadata")
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError("Oversized AetherDesk metadata")
    return data


def remove_tree(path):
    # Never traverse a symlink, another filesystem or a mount in private state.
    root = owned(path, True)
    count = 0
    for parent, directories, files in os.walk(str(path), followlinks=False):
        if len(Path(parent).relative_to(path).parts) > 8:
            raise ValueError("Unexpectedly deep temporary tree")
        for name in directories + files:
            item = Path(parent) / name
            info = item.lstat()
            count += 1
            if count > 4096 or info.st_uid != os.geteuid() or info.st_dev != root.st_dev or os.path.ismount(str(item)):
                raise ValueError("Unsafe temporary tree")
    if os.path.ismount(str(path)):
        raise ValueError("Temporary directory is a mount")
    if not shutil.rmtree.avoids_symlink_attacks:
        raise ValueError("Safe temporary cleanup is unavailable")
    shutil.rmtree(str(path))


def lock_file(path):
    fd = os.open(str(path), os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    stream = os.fdopen(fd, "r+b")
    if os.fstat(fd).st_uid != os.geteuid() or not stat.S_ISREG(os.fstat(fd).st_mode):
        stream.close()
        raise ValueError("Invalid lease file")
    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    return stream


def connection_info(path):
    if not UUID.fullmatch(path.name):
        raise ValueError("Unknown connection directory")
    owned(path, True)
    info = json.loads(small(path / ".owned.json"))
    if not isinstance(info, dict) or info.get("owner") != MAGIC or info.get("id") != path.name or not VERSION.fullmatch(info.get("version", "")):
        raise ValueError("Unknown connection ownership")
    return info


def sweep_connections(base):
    active = set()
    paths = list(base.iterdir())
    if len(paths) > 4096:
        return None
    for path in paths:
        try:
            info = connection_info(path)
            try:
                lease = lock_file(path / ".lease")
            except BlockingIOError:
                active.add(info["version"])
                continue
            with lease:
                # An unlocked marker proves an earlier lease ended. Unknown
                # directories and pre-existing non-lease files are left alone.
                remove_tree(path)
        except (OSError, ValueError, TypeError):
            continue
    return active


def cache_files(path):
    owned(path, True)
    if small(path / ".aetherdesk-owned") != CACHE_MAGIC:
        raise ValueError("Unmanaged agent cache")
    manifest = json.loads(small(path / "runtime-manifest.json", 1024 * 1024))
    if not isinstance(manifest, dict):
        raise ValueError("Unknown runtime manifest")
    entries = manifest.get("files", [])
    if manifest.get("schema_version") != 1 or not isinstance(entries, list) or not 0 < len(entries) <= 128 or any(not isinstance(entry, dict) for entry in entries):
        raise ValueError("Unknown runtime manifest")
    names = {entry["file_name"] for entry in entries}
    if len(names) != len(entries) or any(not re.fullmatch(r"[A-Za-z0-9_.+-]+", n) or n in (".", "..") for n in names):
        raise ValueError("Invalid runtime name")
    if {p.name for p in path.iterdir()} != {"ad-agent", "libs", "runtime-manifest.json", ".aetherdesk-owned"}:
        raise ValueError("Cache contains unknown or resumable files")
    owned(path / "libs", True)
    if {p.name for p in (path / "libs").iterdir()} != names:
        raise ValueError("Cache contains unknown libraries")
    files = [path / "ad-agent", path / "runtime-manifest.json", path / ".aetherdesk-owned"]
    files += [path / "libs" / name for name in names]
    for item in files:
        owned(item)
    return files


def live_cache_versions(base, versions):
    active = set()
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if proc.stat().st_uid != os.geteuid():
                continue
            executable = os.readlink(str(proc / "exe"))
            with (proc / "maps").open("rb") as stream:
                maps = stream.read(2 * 1024 * 1024 + 1)
            if len(maps) > 2 * 1024 * 1024:
                return None
            for version in versions:
                prefix = str(base / version) + "/"
                if prefix in executable or prefix.encode() in maps:
                    active.add(version)
        except FileNotFoundError:
            if proc.exists():
                return None
            continue
        except OSError:
            # If procfs hides a live process, do not guess that its cache is idle.
            if proc.exists():
                return None
    return active


def prune_caches(root, current, active):
    base = root / "agent"
    if not base.exists():
        return True
    owned(base, True)
    versions = [p.name for p in base.iterdir() if VERSION.fullmatch(p.name)]
    if len(versions) > 128:
        return False
    live = live_cache_versions(base, versions)
    if live is None:
        return False
    keep = set(sorted(versions, key=lambda v: tuple(int(n) for n in v.split(".")), reverse=True)[:2])
    keep.update(active | live | {current})
    cutoff = time.time() - 7 * 86400
    for version in set(versions) - keep:
        try:
            path = base / version
            files = cache_files(path)
            if all(owned(p).st_mtime < cutoff for p in files):
                remove_tree(path)
        except (OSError, ValueError, KeyError, TypeError):
            continue
    return True


def prune_diagnostics(root):
    base = root / "diagnostics"
    if not base.exists():
        return
    owned(base, True)
    records = []
    for path in list(base.iterdir())[:4096]:
        try:
            if path.suffix != ".log" or not UUID.fullmatch(path.stem):
                continue
            meta = owned(path)
            if meta.st_size > 16 * 1024:
                continue
            with path.open("rb") as stream:
                if stream.read(len(b"AETHERDESK_DIAGNOSTIC_V1\n")) != b"AETHERDESK_DIAGNOSTIC_V1\n":
                    continue
            records.append((meta.st_mtime, path))
        except (OSError, ValueError):
            continue
    for index, (modified, path) in enumerate(sorted(records, reverse=True)):
        if index >= 20 or time.time() - modified > 7 * 86400:
            path.unlink()


def sweep_ended(root):
    base = root / "sessions"
    if not base.exists():
        return
    owned(base, True)
    for path in list(base.iterdir())[:4096]:
        try:
            if not UUID.fullmatch(path.name):
                continue
            owned(path, True)
            # Written only after supervisor shutdown, before best-effort removal.
            if small(path / "ended") == ("AETHERDESK_SESSION_ENDED_V1 " + path.name + "\n").encode():
                remove_tree(path)
        except (OSError, ValueError):
            continue


def main():
    mode, identity, version = sys.argv[1:4]
    if not UUID.fullmatch(identity) or not VERSION.fullmatch(version):
        raise ValueError("Invalid cleanup identity")
    os.umask(0o077)
    root = private_dir(Path.home() / ".aetherdesk")
    if mode == "mark-cache":
        path = root / "agent" / version
        owned(root / "agent", True)
        owned(path, True)
        owned(path / "runtime-manifest.json")
        target = path / ".aetherdesk-owned"
        fd = os.open(str(target), os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(CACHE_MAGIC)
        return
    if mode != "lease":
        raise ValueError("Unknown cleanup mode")
    base = private_dir(root / "connections")
    path = base / identity
    path.mkdir(mode=0o700)  # Unique per SSH connection; never adopt another tree.
    lease = lock_file(path / ".lease")
    (path / ".owned.json").write_text(json.dumps({"owner": MAGIC, "id": identity, "version": version}))
    def stop(_signal, _frame):
        raise SystemExit(0)
    signal.signal(signal.SIGHUP, stop)
    signal.signal(signal.SIGTERM, stop)
    try:
        print("AETHERDESK_CLEANUP_READY_V1=" + str(path), flush=True)
        # Holding the lock makes this connection invisible to other sweepers.
        # Failure is non-fatal; a later connection retries inaccessible residue.
        try:
            with lock_file(root / ".maintenance-lock"):
                active = sweep_connections(base)
                sweep_ended(root)
                prune_diagnostics(root)
                stamp = root / ".cache-maintenance"
                recent = stamp.exists() and time.time() - owned(stamp).st_mtime < 86400
                if active is not None and not recent:
                    if prune_caches(root, version, active):
                        # The timestamp contains no credentials and is advisory.
                        fd = os.open(str(stamp), os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
                        os.close(fd)
        except (OSError, ValueError):
            pass
        sys.stdin.buffer.read(1)
    finally:
        try:
            remove_tree(path)
        except (OSError, ValueError):
            pass
        lease.close()


def failure_code(error):
    if isinstance(error, OSError):
        if error.errno in (errno.ENOSPC, getattr(errno, "EDQUOT", -1)):
            return "storage_full"
        if error.errno in (errno.EACCES, errno.EPERM):
            return "permission_denied"
        return "filesystem_error"
    return "unsafe_state"


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        # Never print environment values, paths supplied by commands or secrets.
        print("AETHERDESK_CLEANUP_ERROR_V1=" + failure_code(error), file=sys.stderr, flush=True)
        sys.exit(1)
