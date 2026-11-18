#!/usr/bin/env python3
"""Read-only agent inventory and optional side-by-side staging. Never stops sessions.

Run on Linux as the session owner. Without --stage this only reports a plan.
--all-users is inventory-only; staging never writes into another account's home.
"""
import argparse
import ctypes
import errno
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import stat
import struct
import uuid


DOMAIN = b"AetherDesk agent deployment v1\0"
NAME = re.compile(r"[A-Za-z0-9_.-]{1,64}\Z")
VERSION = re.compile(r"[0-9]+(?:\.[0-9]+){1,2}\Z")
HEX = re.compile(r"[0-9a-fA-F]{64}\Z")
MAX_FILE = 512 * 1024 * 1024


def regular(path):
    s = path.lstat()
    if not stat.S_ISREG(s.st_mode) or not 0 < s.st_size <= MAX_FILE:
        raise ValueError("Bundle contains a link, special, empty or oversized file")
    return s


def digest(path):
    regular(path)
    h = hashlib.sha256()
    with path.open("rb") as f:
        for b in iter(lambda: f.read(1024 * 1024), b""):
            h.update(b)
    return h.hexdigest()


def bundle_identity(agent_hash, manifest):
    h = hashlib.sha256(DOMAIN)
    h.update(bytes.fromhex(agent_hash))
    for item in sorted(manifest["files"], key=lambda i: i["file_name"]):
        h.update(item["file_name"].encode("ascii") + b"\0")
        h.update(struct.pack(">Q", item["size"]))
        h.update(item["sha256"].lower().encode("ascii"))
    return h.hexdigest()


def inspect_bundle(directory):
    directory = Path(directory).absolute()
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("Bundle must be a real directory")
    directory = directory.resolve(strict=True)
    version_path = directory / "version.txt"
    if regular(version_path).st_size > 128:
        raise ValueError("Invalid bundle version")
    version = version_path.read_text().strip()
    if not VERSION.fullmatch(version) or len(version) > 64:
        raise ValueError("Invalid bundle version")
    manifest_path = directory / "runtime-manifest.json"
    if regular(manifest_path).st_size > 1024 * 1024:
        raise ValueError("Oversized runtime manifest")
    manifest = json.loads(manifest_path.read_text())
    items = manifest.get("files")
    if manifest.get("schema_version") != 1 or not isinstance(items, list) or not 0 < len(items) <= 128:
        raise ValueError("Invalid runtime manifest")
    names = set()
    for item in items:
        name = item.get("file_name", "")
        if not NAME.fullmatch(name) or name in (".", "..") or name in names:
            raise ValueError("Unsafe or duplicate library name")
        names.add(name)
        if type(item.get("size")) is not int or not 0 < item["size"] <= MAX_FILE or not HEX.fullmatch(item.get("sha256", "")):
            raise ValueError("Invalid library identity")
        path = directory / "libs" / name
        if regular(path).st_size != item["size"] or digest(path) != item["sha256"].lower():
            raise ValueError("Library differs from its manifest")
    if (directory / "libs").is_symlink():
        raise ValueError("Linked runtime directory")
    agent_hash = digest(directory / "ad-agent")
    # Retain the supplied canonical licence notices in the staged bundle too.
    notices = directory / "licenses"
    if notices.is_symlink() or not notices.is_dir():
        raise ValueError("Bundle licence notices are required")
    files = ["ad-agent", "version.txt", "runtime-manifest.json"] + ["libs/" + n for n in sorted(names)]
    for base, dirs, leaves in os.walk(str(notices), followlinks=False):
        for d in dirs:
            if (Path(base) / d).is_symlink():
                raise ValueError("Linked licence directory")
        for name in leaves:
            path = Path(base) / name
            regular(path)
            files.append(str(path.relative_to(directory)))
            if len(files) > 4096:
                raise ValueError("Too many bundle files")
    if len(files) == 3 + len(names):
        raise ValueError("Bundle licence notices are empty")
    return {"root": directory, "version": version, "agent_sha256": agent_hash,
            "manifest": manifest, "id": bundle_identity(agent_hash, manifest), "files": files}


def owned_directory(path):
    s = path.lstat()
    if not stat.S_ISDIR(s.st_mode) or s.st_uid != os.geteuid():
        raise ValueError("Destination is linked, not a directory, or belongs to another account")


def destination(home, bundle, create=False):
    home = Path(home).absolute()
    if home != home.resolve(strict=True):
        raise ValueError("Home path must not contain links")
    owned_directory(home)
    path = home
    for part in (".aetherdesk", "agent", bundle["version"], "bundles"):
        path = path / part
        if path.exists() or path.is_symlink():
            owned_directory(path)
        elif create:
            path.mkdir(mode=0o700)
    return path / bundle["id"]


def verify_published(path, bundle):
    owned_directory(path)
    owned_directory(path / "libs")
    if digest(path / "ad-agent") != bundle["agent_sha256"]:
        raise ValueError("Existing bundle differs from its identity; it was preserved")
    for item in bundle["manifest"]["files"]:
        library = path / "libs" / item["file_name"]
        if regular(library).st_size != item["size"] or digest(library) != item["sha256"].lower():
            raise ValueError("Published runtime differs from its identity; it was preserved")
    # Client-staged bundles need not contain version/notices on the host. Leave
    # such existing directories untouched; the client's supplied notices remain.
    return {"id": bundle["id"]}


def cleanup_stage(path, parent, identity):
    if path.parent != parent or not path.name.startswith(".stage-") or path.resolve() != path:
        raise ValueError("Unsafe staging cleanup target")
    owned_directory(parent)
    owned_directory(path)
    if os.path.ismount(str(path)):
        raise ValueError("Staging root is a mount point; preserved")
    marker = path / ".stage-owner.json"
    regular(marker)
    if json.loads(marker.read_text()) != {"project": "aetherdesk", "stage": identity, "uid": os.geteuid()}:
        raise ValueError("Staging ownership changed; preserved")
    for base, dirs, files in os.walk(str(path), followlinks=False):
        for d in dirs:
            p = Path(base) / d
            owned_directory(p)
            if os.path.ismount(str(p)):
                raise ValueError("Mount inside staging directory; preserved")
        for f in files:
            s = (Path(base) / f).lstat()
            if not stat.S_ISREG(s.st_mode) or s.st_uid != os.geteuid():
                raise ValueError("Unknown staging file; preserved")
    shutil.rmtree(str(path))


def stage_bundle(home, bundle):
    target = destination(home, bundle, create=True)
    if target.exists() or target.is_symlink():
        verify_published(target, bundle)
        return {"staged": False, "existing_preserved": True, "path": str(target)}
    identity = uuid.uuid4().hex
    work = target.parent / (".stage-" + identity)
    work.mkdir(mode=0o700)
    marker = work / ".stage-owner.json"
    marker.write_text(json.dumps({"project": "aetherdesk", "stage": identity, "uid": os.geteuid()}))
    try:
        for relative in bundle["files"]:
            source = bundle["root"] / relative
            expected = digest(source)
            out = work / relative
            out.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with source.open("rb") as src, out.open("xb") as dst:
                shutil.copyfileobj(src, dst, 1024 * 1024)
                dst.flush()
                os.fsync(dst.fileno())
            if digest(out) != expected:
                raise ValueError("Source changed during staging")
            out.chmod(0o500 if relative == "ad-agent" or relative.startswith("libs/") else 0o400)
        checked = inspect_bundle(work)
        if checked["id"] != bundle["id"]:
            raise ValueError("Bundle changed during staging")
        # Atomic no-replace publication, including concurrent staging processes.
        # Linux/glibc 2.28 supports this; refuse instead of using an unsafe fallback.
        libc = ctypes.CDLL(None, use_errno=True)
        rename = libc.renameat2
        rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        rename.restype = ctypes.c_int
        if rename(-100, os.fsencode(str(work)), -100, os.fsencode(str(target)), 1) != 0:
            code = ctypes.get_errno()
            if code == errno.EEXIST:
                verify_published(target, bundle)
                return {"staged": False, "existing_preserved": True, "path": str(target)}
            raise OSError(code, "Atomic no-replace bundle publication failed")
        return {"staged": True, "path": str(target), "running_sessions_preserved": True}
    finally:
        if work.exists():
            cleanup_stage(work, target.parent, identity)


def inventory(all_users=False):
    result = []
    hashes = {}
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            uid = proc.stat().st_uid
            if not all_users and uid != os.geteuid():
                continue
            if (proc / "comm").read_text().strip() != "ad-agent":
                continue
            row = {"pid": int(proc.name), "uid": uid, "action": "preserve"}
            try:
                row["account"] = pwd.getpwuid(uid).pw_name
            except KeyError:
                row["account"] = "unknown"
            try:
                args = (proc / "cmdline").read_bytes().split(b"\0")
                if b"--session-id" in args:
                    row["session"] = str(uuid.UUID(args[args.index(b"--session-id") + 1].decode("ascii")))
                row["role"] = "broker" if b"--serve" in args else "watcher_or_helper"
                exe = proc / "exe"
                s = exe.stat()
                key = (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns)
                if key not in hashes:
                    h = hashlib.sha256()
                    with exe.open("rb") as f:
                        for b in iter(lambda: f.read(1024 * 1024), b""):
                            h.update(b)
                    hashes[key] = h.hexdigest()
                row["agent_sha256"] = hashes[key]
            except (OSError, ValueError, IndexError, UnicodeError):
                row["identity"] = "unverified; preserve"
            result.append(row)
        except (OSError, ValueError):
            continue
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, help="Linux agent bundle with runtime manifest, version and licence notices")
    parser.add_argument("--stage", action="store_true", help="Stage alongside existing runtimes; does not restart or connect sessions")
    parser.add_argument("--all-users", action="store_true", help="Read-only inventory; unreadable identities remain unknown")
    args = parser.parse_args()
    if args.stage and (not args.bundle or args.all_users):
        parser.error("--stage requires --bundle and cannot be used with --all-users")
    if not sys_platform_linux():
        parser.error("Run this tool on the Linux host as the session owner")
    bundle = inspect_bundle(args.bundle) if args.bundle else None
    home = Path(pwd.getpwuid(os.geteuid()).pw_dir)
    report = {"project": "aetherdesk", "dry_run": not args.stage, "processes": inventory(args.all_users),
              "policy": "No process signals, session termination, runtime replacement, cleanup or settings changes. Reconnecting a retained session keeps its old agent. Owners save work before explicitly ending old sessions."}
    if bundle:
        report["bundle"] = {"id": bundle["id"], "version": bundle["version"], "agent_sha256": bundle["agent_sha256"], "destination": str(destination(home, bundle))}
        if args.stage:
            report["staging"] = stage_bundle(home, bundle)
    print(json.dumps(report, indent=2))


def sys_platform_linux():
    import sys
    return sys.platform.startswith("linux")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, AttributeError, KeyError, TypeError) as error:
        raise SystemExit("Agent staging stopped safely: " + str(error))
