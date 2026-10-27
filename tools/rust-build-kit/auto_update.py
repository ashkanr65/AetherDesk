"""Publish a completed AetherDesk build. Never compiles or installs the application.

Normal execution commits reviewed source to main and publishes a stable update.
Use --check for local verification only, or --draft to stop before publication.
Signing uses the existing protected key; credentials never enter files or logs.
"""
import argparse
import base64
from contextlib import contextmanager
import ctypes
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
import sys
import time
from datetime import datetime, timezone
import urllib.error
import urllib.parse
import urllib.request
import zipfile

import update_release as signing
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

ROOT = Path(__file__).resolve().parents[2]
PRIVATE_NAMES = {"monitoring.json", "sessions.json", "sessions.json.bak",
                 "settings.json", "preferences.json", "providers.json", "vault.json",
                 "credentials.json", "id_rsa", "id_ed25519", "ai-history.json",
                 "ai-history-index.json", "desktop-recovery.json", "transfer-queue.json",
                 "terminal-settings.json", "remote-toolbar.json", "editor-language-servers.json"}
PATTERNS = {
    "private key": r"-----BEGIN (?:RSA |EC |OPENSSH |DSA |ENCRYPTED )?PRIVATE KEY-----",
    "access token": r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|sk-(?:proj-)?[A-Za-z0-9_-]{24,})",
    "signed licence data": r'\bSIGN2?\s*=\s*"?[A-Fa-f0-9]{12,}',
    "raw licence report": r"License\s+server\s+status:|lmstat\s+-\s+Copyright",
    "private network address": r"\b(?:10(?:\.[0-9]{1,3}){3}|192\.168(?:\.[0-9]{1,3}){2}|172\.(?:1[6-9]|2[0-9]|3[01])(?:\.[0-9]{1,3}){2})\b",
    "personal Windows path": r"\b[A-Za-z]:[/\\]Users[/\\](?!Public\b|Default\b)[A-Za-z0-9_. -]+[/\\]",
}


class Stop(RuntimeError):
    """A safe, user-facing failure, without HTTP bodies or credential output."""


def require(condition, message):
    if not condition:
        raise Stop(message)


def read_json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, ValueError):
        raise Stop("Missing or invalid JSON file: " + str(path)) from None


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def version_key(version, build):
    require(isinstance(version, str) and re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,2}", version),
            "Use a numeric version in release.json.")
    require(type(build) is int and 0 < build <= 4294967295, "Build must be a positive integer.")
    parts = [int(p) for p in version.split(".")]
    return tuple(parts + [0] * (3 - len(parts)) + [build])


def contained(path, directory):
    return path == directory or directory in path.parents


def no_links(path):
    for item in (path, *path.parents):
        if item.exists():
            info = item.lstat()
            require(not stat.S_ISLNK(info.st_mode) and not getattr(info, "st_file_attributes", 0) & 0x400,
                    "Refusing a linked release/output path: " + str(item))


def private_file(path):
    return (path.name.lower() in PRIVATE_NAMES or path.suffix.lower() in {".key", ".pfx", ".p12", ".lic"}
            or path.name == ".env" or path.name.startswith(".env.") or ".ssh" in path.parts)


def regular_tree(directory):
    """Reject links/junctions before traversing, including on Python 3.9."""
    pending = [directory]
    result = {}
    while pending:
        path = pending.pop()
        info = path.lstat()
        require(not stat.S_ISLNK(info.st_mode) and not getattr(info, "st_file_attributes", 0) & 0x400,
                "Links or junctions are not allowed in a package: " + str(path))
        if stat.S_ISDIR(info.st_mode):
            pending.extend(path.iterdir())
        else:
            require(stat.S_ISREG(info.st_mode), "Unexpected package entry: " + str(path))
            name = path.relative_to(directory).as_posix()
            require(signing.safe_path(name), "Unsafe package filename: " + name)
            require(not private_file(path),
                    "Personal configuration or credential file in package: " + name)
            result[name] = path
    return result


def fingerprint(root, output):
    """Use the build kit's exact source enumeration and hashing, without Cargo."""
    powershell = shutil.which("powershell.exe") or shutil.which("pwsh")
    require(powershell, "Windows PowerShell is required to verify the build fingerprint.")
    script = "$ErrorActionPreference='Stop'; . (Join-Path $env:AETHERDESK_PUBLISH_SOURCE 'tools/rust-build-kit/Build-Common.ps1'); Get-SourceFingerprint"
    env = dict(os.environ, AETHERDESK_PUBLISH_SOURCE=str(root), AETHERDESK_RUST_OUTPUT_ROOT=str(output))
    env.pop("AETHERDESK_RUST_FROZEN_FINGERPRINT", None)
    result = subprocess.run([powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                             "-EncodedCommand", base64.b64encode(script.encode("utf-16-le")).decode()],
                            cwd=root, env=env, capture_output=True, timeout=120)
    value = result.stdout.decode("utf-8-sig", errors="replace").strip()
    require(result.returncode == 0 and re.fullmatch(r"[a-f0-9]{64}", value),
            "Cannot calculate the build fingerprint. Check the build-kit paths and PowerShell.")
    return value


@contextmanager
def build_lock():
    require(os.name == "nt", "Run this publisher on the Windows build computer.")
    from ctypes import wintypes
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.CreateMutexW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    api.CreateMutexW.restype = wintypes.HANDLE
    api.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    api.WaitForSingleObject.restype = wintypes.DWORD
    api.ReleaseMutex.argtypes = [wintypes.HANDLE]
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = api.CreateMutexW(None, False, "Local\\AetherDeskRustPreviewBuild")
    require(handle, "Cannot acquire the build/publisher lock.")
    acquired = api.WaitForSingleObject(handle, 0) in (0, 0x80)
    try:
        require(acquired, "A build or another publisher is running. Wait for it to finish.")
        yield
    finally:
        if acquired:
            api.ReleaseMutex(handle)
        api.CloseHandle(handle)


class Git:
    def __init__(self, root):
        self.root = root

    def run(self, *args, data=None):
        result = subprocess.run(["git", "-C", str(self.root), *args], input=data,
                                capture_output=True, timeout=180, env=dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never"))
        require(result.returncode == 0, "Git operation failed: " + args[0] + ". Review Git status/sign-in; no reset or force-push was attempted.")
        return result.stdout.decode("utf-8", errors="strict").rstrip("\r\n")

    def head(self):
        return self.run("rev-parse", "HEAD")

    def pending(self):
        tracked = self.run("diff", "--name-only", "--no-renames", "-z", "HEAD").split("\0")
        untracked = self.run("ls-files", "--others", "--exclude-standard", "-z").split("\0")
        return sorted(set(n for n in tracked + untracked if n))

    def validate(self, repository):
        require(self.run("branch", "--show-current") == "main", "Switch to main before publishing. No branch will be created.")
        for flag in ((), ("--push",)):
            origin = self.run("remote", "get-url", *flag, "origin")
            require(origin in ("https://github.com/" + repository, "https://github.com/" + repository + ".git",
                               "git@github.com:" + repository + ".git", "git@github.com:" + repository),
                    "The origin remote does not match update-config.json.")
        require(not self.run("ls-files", "-u"), "Resolve merge conflicts before publishing.")

    def audit(self):
        files = self.pending()
        for name in files:
            path = self.root / name
            require(contained(path.resolve(), self.root), "Source link escapes the repository: " + name)
            require(not private_file(path) and path.suffix.lower() != ".log",
                    "Personal data or credentials must not be committed: " + name)
            if not path.exists():
                continue  # Record intentional deletions; never restore user files.
            require(not path.is_symlink() and path.stat().st_size <= 10 * 1024 * 1024,
                    "Review this large or linked source file manually: " + name)
            raw = path.read_bytes()
            if b"\0" in raw:
                require(path.suffix.lower() in {".png", ".ico", ".jpg", ".jpeg", ".gif", ".webp", ".woff", ".woff2"},
                        "Unexpected binary in pending source: " + name)
                continue
            # Scan additions; previously published documentation is not a new secret.
            tracked = self.run("ls-files", "--", name)
            text = "\n".join(line[1:] for line in self.run("diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--unified=0", "HEAD", "--", name).splitlines()
                             if line.startswith("+") and not line.startswith("+++")) if tracked else raw.decode("utf-8-sig")
            text = text.replace(r"\n", "\n").replace(r"\t", "\t")
            for category, pattern in PATTERNS.items():
                require(not re.search(pattern, text), "Possible " + category + " in " + name + ". Remove it or review it manually; the value was not logged.")
            for endpoint in re.findall(r"\b[0-9]{2,5}@([A-Za-z0-9.-]+)", text):
                require(endpoint == "localhost" or endpoint.endswith((".example", ".example.com", ".example.org", ".test", ".invalid")),
                        "Replace a real-looking licence server address with a fictional example in " + name + ". The address was not logged.")
        return files

    def push(self, repository, tag, checked_inputs):
        self.validate(repository)
        self.run("fetch", "origin", "main")
        self.run("merge-base", "--is-ancestor", "origin/main", "HEAD")
        files = self.audit()
        checked_inputs()
        if files:
            self.run("add", "--", *files)
            self.run("commit", "-m", "Release " + tag + " from verified build")
        require(not self.pending(), "Source changed during commit. Rebuild/review before continuing.")
        checked_inputs()
        self.validate(repository)
        self.run("push", "origin", "HEAD:refs/heads/main")
        return self.head()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        raise Stop("Authenticated GitHub API redirected unexpectedly; check the repository address.")


class HTTPSRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        require(urllib.parse.urlsplit(new_url).scheme == "https", "Refusing an insecure download redirect.")
        return super().redirect_request(request, fp, code, message, headers, new_url)


class ProgressReader:
    def __init__(self, stream, total, label, log):
        self.stream, self.total, self.label, self.log = stream, total, label, log
        self.count, self.last = 0, time.monotonic()

    def read(self, size=-1):
        data = self.stream.read(size)
        self.count += len(data)
        if self.log and time.monotonic() - self.last >= 5:
            self.log("{}: {:.0f}% ({:.1f} / {:.1f} MiB)".format(self.label, 100 * self.count / max(self.total, 1), self.count / 1048576, self.total / 1048576))
            self.last = time.monotonic()
        return data


class GitHub:
    def __init__(self, repository):
        self.repository = repository
        result = subprocess.run(["git", "credential", "fill"], input=b"protocol=https\nhost=github.com\n\n",
                                capture_output=True, timeout=60, env=dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never"))
        credential = dict(line.split("=", 1) for line in result.stdout.decode().splitlines() if "=" in line)
        self.token = credential.get("password")
        require(result.returncode == 0 and self.token, "GitHub sign-in is missing. Sign in through Git Credential Manager first.")
        self.prefix = "/repos/" + repository

    def headers(self):
        return {"Authorization": "Bearer " + self.token, "Accept": "application/vnd.github+json",
                "User-Agent": "AetherDesk-publisher", "X-GitHub-Api-Version": "2022-11-28"}

    def api(self, path, method="GET", body=None, missing=False):
        require(path.startswith(self.prefix), "Unexpected GitHub API route.")
        headers = self.headers()
        headers["Content-Type"] = "application/json"
        request = urllib.request.Request("https://api.github.com" + path, method=method, headers=headers,
                                         data=json.dumps(body).encode() if body is not None else None)
        try:
            with urllib.request.build_opener(NoRedirect()).open(request, timeout=60) as response:
                raw = response.read(16 * 1024 * 1024 + 1)
                require(len(raw) <= 16 * 1024 * 1024, "GitHub response was too large.")
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            if error.code == 404 and missing:
                return None
            raise Stop("GitHub " + method + " failed (HTTP " + str(error.code) + "). Re-run the same command after fixing access/network issues.") from None
        except (OSError, ValueError):
            raise Stop("GitHub request failed. Re-run the same command; an existing matching draft will be resumed.") from None

    def project(self):
        project = self.api(self.prefix)
        require(project.get("private") is False and project.get("default_branch") == "main" and not project.get("archived"),
                "Updates require the active public source repository with main as its default branch.")

    def find(self, tag):
        release = self.api(self.prefix + "/releases/tags/" + tag, missing=True)
        if release:
            return release
        # Draft tags need not exist as Git refs yet. Use the authenticated list,
        # as the manual publisher does, before creating a potentially duplicate.
        for page in range(1, 21):
            releases = self.api(self.prefix + "/releases?per_page=100&page=" + str(page))
            matches = [r for r in releases if r.get("tag_name") == tag]
            require(len(matches) <= 1, "Multiple matching drafts exist; inspect them before retrying.")
            if matches:
                return matches[0]
            if len(releases) < 100:
                return None
        raise Stop("Release history exceeded the bounded lookup; inspect the intended tag manually.")

    def upload(self, release_id, path, log=None):
        headers = self.headers()
        headers.update({"Content-Type": "application/octet-stream", "Content-Length": str(path.stat().st_size)})
        connection = http.client.HTTPSConnection("uploads.github.com", timeout=120, blocksize=1024 * 1024)
        try:
            with path.open("rb") as stream:
                connection.request("POST", self.prefix + "/releases/" + str(release_id) + "/assets?name=" + urllib.parse.quote(path.name),
                                   body=ProgressReader(stream, path.stat().st_size, "Uploading " + path.name, log), headers=headers)
                response = connection.getresponse()
                require(response.status == 201, "Asset upload failed (HTTP " + str(response.status) + "). Re-run to resume the draft.")
                return json.loads(response.read(1024 * 1024))
        except OSError:
            raise Stop("Asset upload was interrupted. Re-run the same command to inspect/resume the draft.") from None
        finally:
            connection.close()

    def anonymous(self, path, binary=False):
        request = urllib.request.Request("https://api.github.com" + path,
            headers={"Accept": "application/octet-stream" if binary else "application/vnd.github+json",
                     "User-Agent": "AetherDesk-public-update-verification", "X-GitHub-Api-Version": "2022-11-28"})
        try:
            return urllib.request.build_opener(HTTPSRedirect()).open(request, timeout=60)
        except (OSError, urllib.error.HTTPError):
            raise Stop("Public verification failed. The release may already be published; re-run to verify it without publishing a duplicate.") from None


def verify_assets(release, assets, commit):
    require(release.get("target_commitish") == commit, "Existing release targets a different source revision; it will not be overwritten.")
    remote = {a["name"]: a for a in release.get("assets", [])}
    require(len(remote) == len(release.get("assets", [])) and set(remote) == set(assets), "Release assets are incomplete or unexpected.")
    for name, item in assets.items():
        actual = remote[name]
        require(actual.get("state") == "uploaded" and actual.get("size") == item["bytes"]
                and actual.get("digest") == "sha256:" + item["sha256"], "Remote asset differs from signed files: " + name)


def file_version(path):
    """Read the installer's numeric Windows version; never execute it."""
    from ctypes import wintypes
    api = ctypes.WinDLL("version", use_last_error=True)
    api.GetFileVersionInfoSizeW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(wintypes.DWORD)]
    api.GetFileVersionInfoSizeW.restype = wintypes.DWORD
    api.GetFileVersionInfoW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
    api.VerQueryValueW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(wintypes.UINT)]
    unused = wintypes.DWORD()
    size = api.GetFileVersionInfoSizeW(str(path), ctypes.byref(unused))
    require(0 < size <= 1024 * 1024, "Installer has no valid version resource.")
    data = ctypes.create_string_buffer(size)
    require(api.GetFileVersionInfoW(str(path), 0, size, data), "Cannot read installer version.")
    pointer, length = ctypes.c_void_p(), wintypes.UINT()
    require(api.VerQueryValueW(data, "\\", ctypes.byref(pointer), ctypes.byref(length)) and length.value >= 52, "Invalid installer version.")
    fields = struct.unpack("<13I", ctypes.string_at(pointer, 52))
    require(fields[0] == 0xFEEF04BD, "Invalid installer version signature.")
    return (fields[2] >> 16, fields[2] & 65535, fields[3] >> 16, fields[3] & 65535)


def verify_built_zip(archive_path, package):
    actual = regular_tree(package)
    prefix = package.name + "/"
    with zipfile.ZipFile(archive_path) as archive:
        entries = [entry for entry in archive.infolist() if not entry.is_dir()]
        names = [entry.filename[len(prefix):] if entry.filename.startswith(prefix) else "" for entry in entries]
        require(len(names) == len(set(names)) == len(actual) and set(names) == set(actual), "Original build ZIP and package contain different files.")
        for entry, name in zip(entries, names):
            require(entry.file_size == actual[name].stat().st_size, "Package file changed after ZIP creation: " + name)
            digest = hashlib.sha256()
            with archive.open(entry) as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(block)
            require(digest.hexdigest() == signing.sha(actual[name]), "Package file changed after ZIP creation: " + name)


def verify_inventory(directory, package, installer, config):
    manifest = signing.verify_release(directory, config)
    files = manifest["files"]
    expected = {f["path"]: f for f in files}
    require(len(expected) == len(files) and len({n.lower() for n in expected}) == len(files), "Duplicate signed file paths.")
    actual = regular_tree(package)
    require(set(actual) == set(expected), "Signed inventory and completed package contain different files.")
    for name, path in actual.items():
        require(path.stat().st_size == expected[name]["bytes"] and signing.sha(path) == expected[name]["sha256"],
                "Completed package changed: " + name)
    with zipfile.ZipFile(directory / manifest["portable"]["name"]) as archive:
        entries = archive.infolist()
        require(len(entries) == len(expected) and set(e.filename for e in entries) == set(expected), "ZIP inventory differs from signed inventory.")
        for entry in entries:
            item = expected[entry.filename]
            require(entry.file_size == item["bytes"], "ZIP file length differs: " + entry.filename)
            digest = hashlib.sha256()
            with archive.open(entry) as stream:
                for block in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(block)
            require(digest.hexdigest() == item["sha256"], "ZIP file hash differs: " + entry.filename)
    require(bool(installer) == bool(manifest["installer"]), "Installer selection changed since this signed release was prepared.")
    if installer:
        require(signing.sha(installer) == manifest["installer"]["sha256"], "Installer differs from completed build.")
    return manifest


def verify_public(github, tag, assets, config, log):
    latest = None
    for attempt in range(5):
        with github.anonymous(github.prefix + "/releases/latest") as response:
            data = response.read(1024 * 1024 + 1)
            require(len(data) <= 1024 * 1024, "Oversized public release feed.")
            latest = json.loads(data)
        if latest.get("tag_name") == tag:
            break
        if attempt < 4:
            time.sleep(3)
    require(latest.get("tag_name") == tag and not latest.get("draft") and not latest.get("prerelease"),
            "Public latest feed has not selected this release. Re-run verification; do not create another release.")
    remote = {a["name"]: a for a in latest["assets"]}
    require(set(remote) == set(assets), "Public feed has different assets.")
    public_key = Ed25519PublicKey.from_public_bytes(base64.b64decode(read_json(config)["publicKey"], validate=True))
    for name in sorted(assets, key=lambda n: n != "update.json"):
        expected = assets[name]
        digest, size, chunks = hashlib.sha256(), 0, []
        require(remote[name]["size"] == expected["bytes"], "Public asset size differs: " + name)
        with github.anonymous(github.prefix + "/releases/assets/" + str(remote[name]["id"]), binary=True) as response:
            stream = ProgressReader(response, expected["bytes"], "Verifying " + name, log)
            while True:
                block = stream.read(1024 * 1024)
                if not block:
                    break
                size += len(block)
                require(size <= expected["bytes"], "Public asset exceeds expected size.")
                digest.update(block)
                if name == "update.json":
                    require(size <= 8 * 1024 * 1024, "Oversized update metadata.")
                    chunks.append(block)
        require(size == expected["bytes"] and digest.hexdigest() == expected["sha256"], "Public download hash differs: " + name)
        if name == "update.json":
            envelope = json.loads(b"".join(chunks))
            payload = base64.b64decode(envelope["manifest"], validate=True)
            public_key.verify(base64.b64decode(envelope["signature"], validate=True), payload)
            value = json.loads(payload)
            require("v" + value["version"] + "-build" + str(value["build"]) == tag and value["expires"] > time.time()
                    and value["product"] == "AetherDesk" and value["target"] == "windows-x64", "Public metadata is incompatible or expired.")
        log("Verified anonymous download: " + name)
    return latest


class Publisher:
    def __init__(self, root=ROOT, output=None, check=False, draft=False, portable_only=False):
        self.root = Path(root).resolve()
        self.output = Path(output or os.environ.get("AETHERDESK_RUST_OUTPUT_ROOT") or self.root.parent / "AetherDesk-Rust-output").resolve()
        require(self.output != Path(self.output.anchor) and not contained(self.output, self.root) and not contained(self.root, self.output),
                "Build output must be a separate folder outside the source, not a drive root.")
        self.check, self.draft, self.portable_only = check, draft, portable_only
        no_links(self.output)
        self.git = Git(self.root)
        self.started = time.monotonic()
        self.report = {"status": "Running", "steps": [], "actual_gui_upgrade_tested": False}
        self.evidence = None

    def log(self, message):
        elapsed = int(time.monotonic() - self.started)
        print("[{:02d}:{:02d}] {}".format(elapsed // 60, elapsed % 60, message), flush=True)
        self.report["steps"].append(message)
        if self.evidence:
            write_json(self.evidence / "run.json", self.report)

    def preflight(self):
        self.config = self.root / "update-config.json"
        config = read_json(self.config)
        self.repository = config["repository"]
        require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", self.repository), "Invalid release repository.")
        release = read_json(self.root / "release.json")
        self.version, self.build = release["version"], release["build"]
        key = version_key(self.version, self.build)
        self.version = ".".join(str(n) for n in key[:3])
        self.tag = "v" + self.version + "-build" + str(self.build)
        self.git.validate(self.repository)
        self.git.audit()
        def pointer(name):
            path = Path((self.output / name).read_text(encoding="utf-8-sig").strip())
            require(path.is_absolute() and path.exists() and contained(path.resolve(), self.output / "packages"), "Invalid completed-package pointer: " + name)
            no_links(path)
            return path
        self.package = pointer("LATEST-FOLDER.txt")
        self.original_zip = pointer("LATEST-ZIP.txt")
        self.installer = None if self.portable_only else pointer("LATEST-INSTALLER.txt")
        if self.installer:
            require(self.installer.name == self.package.name + "-Setup.exe" and file_version(self.installer) == key,
                    "Installer pointer/version does not match this exact package build.")
        self.source_fingerprint = fingerprint(self.root, self.output)
        info = signing.verify_evidence(self.package)
        require(info.get("fingerprint") == self.source_fingerprint, "Source changed since the completed build. Build the current source before publishing.")
        require(read_json(self.package / "release.json") == release and info.get("version") == release["version"] and info.get("build") == self.build,
                "Source/package version or build differs. Never relabel an existing executable.")
        require(signing.sha(self.root / "rust-regressions.json") == signing.sha(self.package / "verification/regression-catalog.json"), "Package regression catalog differs from source.")
        for name in ("verification/windows-regressions.json", "agent/linux-x86_64/regression-results.json"):
            evidence = read_json(self.package / name)
            require(evidence.get("schemaVersion") == 1 and type(evidence.get("required")) is int and evidence["required"] > 0
                    and type(evidence.get("registered")) is int and evidence["registered"] >= evidence["required"],
                    "Missing or empty registered-test evidence: " + name)
        timing = read_json(self.output / "reports/build-timing.json")
        steps = {s["step"]: s["status"] for s in timing.get("steps", [])}
        required = range(6 if self.portable_only else 7)
        require(timing.get("status") == "Passed" and all(steps.get(n) == "Passed" for n in required), "Required build stages did not all pass.")
        try:
            started = datetime.fromisoformat(timing["startedUtc"].replace("Z", "+00:00"))
            finished = datetime.fromisoformat(timing["updatedUtc"].replace("Z", "+00:00"))
            built = datetime.fromisoformat(info["built"].replace("Z", "+00:00"))
            require(started <= built <= finished, "Build timing and completed package belong to different runs.")
        except (KeyError, ValueError, TypeError):
            raise Stop("Build timing dates are missing or inconsistent.") from None
        regular_tree(self.package)
        with (self.package / "AetherDesk.exe").open("rb") as stream:
            header = stream.read(64)
            require(len(header) == 64 and header[:2] == b"MZ", "Application is not a Windows executable.")
            stream.seek(struct.unpack_from("<I", header, 60)[0])
            require(stream.read(6) == b"PE\0\0\x64\x86", "Application must be Windows x64.")
        self.notes = (self.root / "RELEASE-NOTES.md").read_text(encoding="utf-8-sig")
        heading = self.notes.splitlines()[0] if self.notes else ""
        require(len(self.notes.encode()) <= 32768 and re.search(r"(?<![0-9.])" + re.escape(self.version) + r"(?![0-9.])", heading)
                and re.search(r"\bbuild\s+" + str(self.build) + r"\b", heading, re.I),
                "Update RELEASE-NOTES.md with this version/build before publishing.")
        self.log("Completed build, exact source fingerprint, evidence, version, source audit and x64 package verified: " + self.tag)

    def checked_inputs(self):
        require(fingerprint(self.root, self.output) == self.source_fingerprint, "Build inputs changed during publication; no unverified source may be pushed.")
        require((self.root / "RELEASE-NOTES.md").read_text(encoding="utf-8-sig") == self.notes, "Release notes changed during publication.")

    def unchanged(self, commit):
        self.git.validate(self.repository)
        require(self.git.head() == commit and not self.git.pending(), "Source changed during publication. The draft was preserved; review/rebuild before retrying.")
        self.checked_inputs()

    def run(self):
        self.preflight()
        if self.check:
            self.log("CHECK PASSED. No signing key, GitHub credentials, commits, pushes or releases were used.")
            return None
        verify_built_zip(self.original_zip, self.package)
        self.log("Original completed-build ZIP matches every package file.")
        self.log("This run publishes a normal update for all users." if not self.draft else "Draft mode: the update will remain unpublished.")
        try:
            key = signing.load_key()
        except Exception:
            raise Stop("Cannot read the existing signing key. Use the original Windows account or restore its original key; no replacement key was created.") from None
        require(signing.public(key) == read_json(self.config)["publicKey"], "Existing signing key differs from pinned key. Recover the original key; do not generate a replacement.")
        del key
        github = GitHub(self.repository)
        github.project()
        existing = github.find(self.tag)
        latest = github.api(github.prefix + "/releases/latest", missing=True)
        if latest and latest["tag_name"] != self.tag:
            match = re.fullmatch(r"v([0-9.]+)-build([0-9]+)", latest["tag_name"])
            require(match and version_key(self.version, self.build) > version_key(match[1], int(match[2])), "Version/build must be newer than the current stable release.")
        directory = self.output / "releases" / (self.version + "-build" + str(self.build))
        self.evidence = self.output / "acceptance" / ("auto-update-" + self.tag)
        no_links(directory)
        no_links(self.evidence)
        self.evidence.mkdir(parents=True, exist_ok=True)
        self.report.update({"tag": self.tag, "source_fingerprint": self.source_fingerprint})
        if not (directory / "update.json").exists():
            require(not existing, "This release already exists but local signed files are missing. Restore them; do not re-sign or overwrite the release.")
            require(not directory.exists() or not any(directory.iterdir()), "Interrupted signing output exists. Preserve it and move it aside before retrying.")
            signing.prepare(self.package, directory, self.installer, self.notes, self.config)
        manifest = verify_inventory(directory, self.package, self.installer, self.config)
        require(manifest["version"] == self.version and manifest["build"] == self.build and manifest["notes"] == self.notes,
                "Saved signed release belongs to different notes/version. Do not overwrite an existing release.")
        self.log("Signature, every ZIP/package file and installer copy verified ({} files).".format(len(manifest["files"])))
        names = [manifest["portable"]["name"], "update.json"]
        if manifest["installer"]:
            names.append(manifest["installer"]["name"])
        assets = {name: signing.asset(directory / name) for name in names}
        if existing:
            commit = self.git.head()
            self.unchanged(commit)
            require(existing.get("target_commitish") == commit, "Existing release targets another source commit. It will not be retargeted.")
            require(github.api(github.prefix + "/git/ref/heads/main")["object"]["sha"] == commit, "Remote main differs from the release source.")
        else:
            self.checked_inputs()
            commit = self.git.push(self.repository, self.tag, self.checked_inputs)
            self.unchanged(commit)
            require(github.api(github.prefix + "/git/ref/heads/main")["object"]["sha"] == commit, "Pushed main revision was not confirmed by GitHub.")
        self.report["source_commit"] = commit
        self.log("Verified source commit on public main: " + commit)
        ref = github.api(github.prefix + "/git/ref/tags/" + self.tag, missing=True)
        if ref:
            require(ref["object"]["type"] == "commit" and ref["object"]["sha"] == commit,
                    "Existing tag points to different source; it will not be changed.")
        if not existing:
            existing = github.api(github.prefix + "/releases", "POST", {"tag_name": self.tag, "target_commitish": commit,
                "name": "AetherDesk " + self.version + " build " + str(self.build), "body": self.notes, "draft": True, "prerelease": False})
        require(existing.get("tag_name") == self.tag and not existing.get("prerelease"), "Release tag/type differs from the intended stable update.")
        if existing.get("draft"):
            remote = {a["name"]: a for a in existing.get("assets", [])}
            require(set(remote) <= set(assets), "Draft contains unexpected assets; inspect it manually.")
            for name, item in assets.items():
                previous = remote.get(name)
                if previous and previous.get("state") == "starter" and previous.get("size") == 0:
                    github.api(github.prefix + "/releases/assets/" + str(previous["id"]), "DELETE")
                    previous = None  # Only discard an empty, incomplete draft upload.
                if previous:
                    require(previous.get("state") == "uploaded" and previous.get("digest") == "sha256:" + item["sha256"] and previous.get("size") == item["bytes"],
                            "Existing draft asset differs; it will not be replaced: " + name)
                else:
                    self.log("Uploading " + name)
                    github.upload(existing["id"], directory / name, self.log)
        existing = github.api(github.prefix + "/releases/" + str(existing["id"]))
        verify_assets(existing, assets, commit)
        self.unchanged(commit)
        require(github.api(github.prefix + "/git/ref/heads/main")["object"]["sha"] == commit, "Remote main changed during upload; publication stopped.")
        github.project()
        ref = github.api(github.prefix + "/git/ref/tags/" + self.tag, missing=True)
        if ref:
            require(ref["object"]["type"] == "commit" and ref["object"]["sha"] == commit, "Release tag changed during upload.")
        if self.draft and existing["draft"]:
            self.report.update(status="Draft verified", url=existing["html_url"])
            self.log("Draft verified; normal automatic updates are not enabled: " + existing["html_url"])
        else:
            if existing["draft"]:
                current = github.api(github.prefix + "/releases/latest", missing=True)
                if current and current["tag_name"] != self.tag:
                    match = re.fullmatch(r"v([0-9.]+)-build([0-9]+)", current["tag_name"])
                    require(match and version_key(self.version, self.build) > version_key(match[1], int(match[2])),
                            "A newer stable release appeared during upload; this draft was preserved.")
                existing = github.api(github.prefix + "/releases/" + str(existing["id"]), "PATCH",
                                      {"draft": False, "prerelease": False, "make_latest": "true"})
            require(not existing.get("draft") and not existing.get("prerelease"), "Release did not become stable.")
            self.log("Published. Verifying anonymous update feed and full downloads; no installation will run.")
            public = verify_public(github, self.tag, assets, self.config, self.log)
            ref = github.api(github.prefix + "/git/ref/tags/" + self.tag)
            require(ref["object"]["type"] == "commit" and ref["object"]["sha"] == commit, "Public release tag points to different source.")
            self.report.update(status="Published and verified", url=public["html_url"], assets=list(assets.values()),
                               signed_inventory_files=len(manifest["files"]))
            self.log("DONE: " + public["html_url"])
        (self.output / "LATEST-SIGNED-RELEASE.txt").write_text(str(directory) + "\n", encoding="utf-8")
        (self.output / "LATEST-RELEASE-CHECK.txt").write_text(str(self.evidence) + "\n", encoding="utf-8")
        write_json(self.evidence / "run.json", self.report)
        return self.report["url"]


def main():
    parser = argparse.ArgumentParser(prog="Prepare-Auto-Update.py", description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="Local preflight only; no credentials or remote operations")
    mode.add_argument("--draft", action="store_true", help="Sign, commit, push and upload, but leave a draft")
    parser.add_argument("--output-root", type=Path, help="Override the sibling build-output folder")
    parser.add_argument("--portable-only", action="store_true", help="Explicitly publish without an installer")
    args = parser.parse_args()
    publisher = None
    try:
        publisher = Publisher(output=args.output_root, check=args.check, draft=args.draft, portable_only=args.portable_only)
        with build_lock():
            publisher.run()
        return 0
    except (KeyboardInterrupt, Exception) as error:
        message = str(error) if isinstance(error, Stop) else "Operation interrupted or failed (" + type(error).__name__ + "). Check prerequisites and re-run the same command."
        if publisher:
            publisher.report.update(status="Stopped", error=message)
            publisher.log("STOPPED: " + message)
            if publisher.evidence:
                print("Verification record: " + str(publisher.evidence), flush=True)
        else:
            print("STOPPED: " + message, flush=True)
        return 1


if __name__ == "__main__":
    sys.exit(main())
