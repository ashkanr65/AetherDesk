"""Reject incomplete editor/runtime packages without launching AetherDesk."""
import argparse
import hashlib
import json
from pathlib import Path


def verify_assets(directory):
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8-sig"))
    files = manifest.get("files", [])
    required = {"index.html", "editor.js", "editor.css", "editor.worker.js", "json.worker.js", "css.worker.js", "html.worker.js", "ts.worker.js"}
    if manifest.get("version") != 1 or not isinstance(files, list) or not all(isinstance(name, str) for name in files):
        raise ValueError("Invalid editor asset manifest")
    if not required.issubset(files) or len(files) != len(set(files)):
        raise ValueError("Editor assets are incomplete or duplicated")
    for name in files:
        if not name or Path(name).name != name or "/" in name or "\\" in name or name in (".", ".."):
            raise ValueError("Unsafe editor asset path")
        if not (directory / name).is_file() or (directory / name).stat().st_size == 0:
            raise ValueError(f"Missing or empty editor asset: {name}")
    if not any(name.endswith(".ttf") for name in files):
        raise ValueError("Monaco icon font is missing")
    return len(files)


def verify_package(directory):
    count = verify_assets(directory / "data/flutter_assets/assets/editor")
    for name in ["webview_windows_plugin.dll", "WebView2Loader.dll", "Install-Editor-Runtime.cmd", "Install-Editor-Runtime.ps1"]:
        if not (directory / name).is_file() or not (directory / name).stat().st_size:
            raise ValueError(f"Editor package is missing {name}")
    installer = directory / "redist/MicrosoftEdgeWebView2RuntimeInstallerX64.exe"
    manifest = json.loads((directory / "redist/webview2-manifest.json").read_text(encoding="utf-8-sig"))
    if installer.stat().st_size < 10000000:
        raise ValueError("The package needs the full offline WebView2 installer")
    digest = hashlib.sha256()
    with installer.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    if manifest.get("schemaVersion") != 1 or digest.hexdigest() != manifest.get("sha256"):
        raise ValueError("Editor runtime checksum does not match its manifest")
    return count


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--assets", type=Path)
    source.add_argument("--package", type=Path)
    args = parser.parse_args()
    try:
        count = verify_assets(args.assets) if args.assets else verify_package(args.package)
        print(f"PASS: {count} offline editor assets" + (" and packaged runtime" if args.package else ""))
    except (OSError, ValueError) as error:
        parser.exit(1, f"Editor packaging: {error}\n")
