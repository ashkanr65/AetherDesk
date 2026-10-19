"""Publisher-only tooling. Python/cryptography are NOT application dependencies.

The private Ed25519 key stays in Windows Credential Manager, or an Actions
secret named UPDATE_SIGNING_KEY (base64 PKCS8 DER). Never place it in source.
"""
import argparse
import base64
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import time
import subprocess
import urllib.request
import zipfile

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

ROOT = Path(__file__).resolve().parents[2]
TARGET = "AetherDesk-Rust/provider/updates-release-signing"


class Credential(ctypes.Structure):
    _fields_ = [("Flags", wintypes.DWORD), ("Type", wintypes.DWORD),
                ("TargetName", wintypes.LPWSTR), ("Comment", wintypes.LPWSTR),
                ("LastWritten", wintypes.FILETIME), ("CredentialBlobSize", wintypes.DWORD),
                ("CredentialBlob", ctypes.POINTER(ctypes.c_ubyte)),
                ("Persist", wintypes.DWORD), ("AttributeCount", wintypes.DWORD),
                ("Attributes", ctypes.c_void_p), ("TargetAlias", wintypes.LPWSTR),
                ("UserName", wintypes.LPWSTR)]


def vault(write=None):
    if os.name != "nt":
        raise ValueError("Use UPDATE_SIGNING_KEY on this publisher machine")
    api = ctypes.WinDLL("advapi32", use_last_error=True)
    api.CredReadW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                             ctypes.POINTER(ctypes.POINTER(Credential))]
    api.CredReadW.restype = wintypes.BOOL
    api.CredWriteW.argtypes = [ctypes.POINTER(Credential), wintypes.DWORD]
    api.CredWriteW.restype = wintypes.BOOL
    api.CredFree.argtypes = [ctypes.c_void_p]
    if write is not None:
        blob = (ctypes.c_ubyte * len(write)).from_buffer_copy(write)
        item = Credential(Type=1, TargetName=TARGET, CredentialBlobSize=len(write),
                          CredentialBlob=blob, Persist=2, UserName="AetherDesk publisher")
        if not api.CredWriteW(ctypes.byref(item), 0):
            raise ValueError("Cannot save the release signing key in Credential Manager")
        return None
    item = ctypes.POINTER(Credential)()
    if not api.CredReadW(TARGET, 1, 0, ctypes.byref(item)):
        if ctypes.get_last_error() == 1168:
            return None
        raise ValueError("Cannot read the release signing key")
    try:
        return ctypes.string_at(item.contents.CredentialBlob, item.contents.CredentialBlobSize)
    finally:
        api.CredFree(item)


def public(key):
    return base64.b64encode(key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw)).decode()


def load_key():
    value = os.environ.get("UPDATE_SIGNING_KEY")
    raw = base64.b64decode(value, validate=True) if value else vault()
    if raw is None:
        raise ValueError("No signing key. Run init-key once on the publisher PC")
    key = serialization.load_der_private_key(raw, password=None)
    if not isinstance(key, Ed25519PrivateKey):
        raise ValueError("Expected an Ed25519 signing key")
    return key


def init_key(config_path):
    config = json.loads(config_path.read_text(encoding="utf-8-sig"))
    raw = vault()
    if raw:
        key = serialization.load_der_private_key(raw, password=None)
    elif config.get("publicKey"):
        raise ValueError("Public key already configured but private key unavailable. Do not replace it; recover the existing publisher key.")
    else:
        key = Ed25519PrivateKey.generate()
        raw = key.private_bytes(serialization.Encoding.DER, serialization.PrivateFormat.PKCS8,
                                serialization.NoEncryption())
        vault(raw)
    if config.get("publicKey") and config["publicKey"] != public(key):
        raise ValueError("Existing signing key does not match the pinned verification key")
    config["publicKey"] = public(key)
    config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    print("Release signing key protected in Windows Credential Manager. Only the public key was written to source.")


def safe_path(path):
    reserved = {"CON", "PRN", "AUX", "NUL"} | {f"{p}{i}" for p in ("COM", "LPT") for i in range(1, 10)}
    return bool(path) and len(path) < 220 and all(
        p not in ("", ".", "..") and not p.endswith((".", " "))
        and not any(ord(c) < 32 or c in '\\:<>"|?*' for c in p)
        and p.split(".")[0].upper() not in reserved for p in path.split("/"))


def sha(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def asset(path):
    return {"name": path.name, "bytes": path.stat().st_size, "sha256": sha(path)}


def verify_evidence(package):
    info = json.loads((package / "build-info.json").read_text(encoding="utf-8-sig"))
    if info.get("runtime") != "native-rust":
        raise ValueError("Only native Rust packages may be published")
    catalog = sha(package / "verification/regression-catalog.json")
    for path in ("verification/windows-regressions.json", "agent/linux-x86_64/regression-results.json"):
        result = json.loads((package / path).read_text(encoding="utf-8-sig"))
        if result.get("compilerTestsPassed") is not True or result.get("catalogSha256", "").lower() != catalog:
            raise ValueError("Package regression evidence is missing or mismatched")
    result = json.loads((package / "verification/x11-runtime.json").read_text(encoding="utf-8-sig"))
    if result.get("bundledX11TestPassed") is not True:
        raise ValueError("Bundled X11 acceptance failed")
    return info


def prepare(package, output, installer, notes, config_path=ROOT / "update-config.json"):
    package = package.resolve(strict=True)
    output = output.resolve()
    if output == package or package in output.parents or output in package.parents:
        raise ValueError("Update release output must be separate from the package")
    if output.exists() and any(output.iterdir()):
        raise ValueError("Choose a new empty release output folder")
    config = json.loads(config_path.read_text(encoding="utf-8-sig"))
    key = load_key()
    if public(key) != config["publicKey"]:
        raise ValueError("Release signing key does not match the key pinned in this build")
    info = verify_evidence(package)
    release = json.loads((package / "release.json").read_text(encoding="utf-8-sig"))
    version = release["version"]
    parts = version.split(".")
    if not 1 <= len(parts) <= 3 or any(not p.isascii() or not p.isdigit() for p in parts):
        raise ValueError("Expected a numeric release version")
    version = ".".join(parts + ["0"] * (3 - len(parts)))
    build = release["build"]
    if info.get("version") != release["version"] or info.get("build") != build:
        raise ValueError("Package version and build information differ")
    files = []
    names = set()
    for path in sorted(package.rglob("*")):
        if path.is_symlink() or (hasattr(path, "is_junction") and path.is_junction()):
            raise ValueError("A package cannot contain links or junctions")
        if not path.is_file():
            continue
        relative = path.relative_to(package).as_posix()
        if not safe_path(relative) or relative.lower() in names:
            raise ValueError("Unsafe or duplicate package path")
        names.add(relative.lower())
        files.append({"path": relative, "bytes": path.stat().st_size, "sha256": sha(path)})
    if "aetherdesk.exe" not in names or not files or len(files) > 30000:
        raise ValueError("Incomplete native application package")
    output.mkdir(parents=True, exist_ok=True)
    zip_path = output / f"AetherDesk-{version}-build{build}-windows-x64.zip"
    # Cargo preserves reproducible licence timestamps dating back to 1970.
    # ZIP starts in 1980: clamp archive metadata, never alter package bytes.
    with zipfile.ZipFile(zip_path, "x", zipfile.ZIP_DEFLATED, compresslevel=1,
                         strict_timestamps=False) as archive:
        for file in files:
            archive.write(package / file["path"], file["path"])
    installer_asset = None
    if installer:
        installer = installer.resolve(strict=True)
        target = output / f"AetherDesk-{version}-build{build}-Setup.exe"
        shutil.copyfile(installer, target)
        installer_asset = asset(target)
    now = int(time.time())
    manifest = {"schema": 1, "product": "AetherDesk", "target": "windows-x64",
                "version": version, "build": build, "published": now, "expires": now + 365 * 86400,
                "notes": notes, "portable": asset(zip_path), "installer": installer_asset, "files": files}
    payload = json.dumps(manifest, separators=(",", ":"), ensure_ascii=False).encode()
    envelope = {"manifest": base64.b64encode(payload).decode(),
                "signature": base64.b64encode(key.sign(payload)).decode()}
    (output / "update.json").write_text(json.dumps(envelope, separators=(",", ":")), encoding="utf-8")
    print(f"Prepared signed update {version} build {build}. No release was published.")
    return manifest


def verify_release(directory, config_path=ROOT / "update-config.json"):
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    config = json.loads(config_path.read_text(encoding="utf-8-sig"))
    metadata_file = directory / "update.json"
    if metadata_file.stat().st_size > 8 * 1024 * 1024:
        raise ValueError("Oversized metadata")
    envelope = json.loads(metadata_file.read_text(encoding="utf-8"))
    payload = base64.b64decode(envelope["manifest"], validate=True)
    Ed25519PublicKey.from_public_bytes(base64.b64decode(config["publicKey"], validate=True)).verify(
        base64.b64decode(envelope["signature"], validate=True), payload)
    manifest = json.loads(payload)
    if (manifest["schema"] != 1 or manifest["product"] != "AetherDesk"
            or manifest["target"] != "windows-x64" or manifest["expires"] <= time.time()):
        raise ValueError("Invalid or expired release")
    for item in (manifest["portable"], manifest["installer"]):
        if item is None:
            continue
        if not safe_path(item["name"]) or "/" in item["name"]:
            raise ValueError("Invalid asset path")
        path = directory / item["name"]
        if path.stat().st_size != item["bytes"] or sha(path) != item["sha256"]:
            raise ValueError("Package hash mismatch")
    print("PASS: signed update and package hashes verified.")
    return manifest


def configure_github():
    """Publisher-only: seal the key directly into the private source repo secret."""
    from nacl.public import PublicKey, SealedBox
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never")
    response = subprocess.run(["git", "credential", "fill"], input="protocol=https\nhost=github.com\n\n",
                              capture_output=True, text=True, env=env, check=True)
    credential = dict(line.split("=", 1) for line in response.stdout.splitlines() if "=" in line)
    token = credential.get("password")
    if not token:
        raise ValueError("Git Credential Manager sign-in required")

    def api(path, method="GET", body=None):
        request = urllib.request.Request("https://api.github.com" + path, method=method,
            data=json.dumps(body).encode() if body is not None else None,
            headers={"Authorization": "Bearer " + token, "Accept": "application/vnd.github+json",
                     "Content-Type": "application/json", "X-GitHub-Api-Version": "2022-11-28"})
        with urllib.request.urlopen(request, timeout=30) as stream:
            data = stream.read()
            return json.loads(data) if data else None

    repo = "/repos/ashkanr65/AetherDesk-Rust"
    if api(repo)["private"] is not True:
        raise ValueError("Signing secrets may only be configured in the private source repository")
    key = load_key()
    config = json.loads((ROOT / "update-config.json").read_text(encoding="utf-8-sig"))
    if public(key) != config["publicKey"]:
        raise ValueError("Pinned key does not match publisher key")
    raw = key.private_bytes(serialization.Encoding.DER, serialization.PrivateFormat.PKCS8,
                            serialization.NoEncryption())
    secret = base64.b64encode(raw)
    receiver = api(repo + "/actions/secrets/public-key")
    encrypted = SealedBox(PublicKey(base64.b64decode(receiver["key"]))).encrypt(secret)
    api(repo + "/actions/secrets/UPDATE_SIGNING_KEY", "PUT",
        {"key_id": receiver["key_id"], "encrypted_value": base64.b64encode(encrypted).decode()})
    print("Publisher signing key configured as an encrypted Actions secret in the private source repository.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("init-key")
    sub.add_parser("configure-github")
    command = sub.add_parser("prepare")
    command.add_argument("--package", required=True, type=Path)
    command.add_argument("--output", required=True, type=Path)
    command.add_argument("--installer", type=Path)
    command.add_argument("--notes-file", required=True, type=Path)
    command = sub.add_parser("verify")
    command.add_argument("--directory", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "init-key":
            init_key(ROOT / "update-config.json")
        elif args.command == "configure-github":
            configure_github()
        elif args.command == "prepare":
            notes = args.notes_file.read_text(encoding="utf-8-sig")
            if len(notes.encode()) > 32768:
                raise ValueError("Release notes are too long")
            prepare(args.package, args.output, args.installer, notes)
        else:
            verify_release(args.directory)
    except Exception:
        # Exceptions from crypto/HTTP environments must never echo key material.
        parser.exit(1, "Release preparation failed. Check signing-key access, matching public key, package evidence and paths.\n")


if __name__ == "__main__":
    main()
