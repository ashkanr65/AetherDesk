# Release version

Edit `release.json` in the AetherDesk repository:

```json
{
  "version": "0.2",
  "build": 1
}
```

`version` accepts `major.minor` or `major.minor.patch`, such as `0.2` or `0.2.3`. Increment `build` for a new published build of the same version. Each number must be at most 65535; build starts at 1. The manifest initially specifies version 0.1, build 1.

The beginner kit automatically synchronizes versions before calculating its source fingerprint and copying the checkout. Changing this manifest invalidates earlier step records, so the next `Build-Everything.cmd` rebuilds all affected artifacts. The external beginner-kit launchers continue to delegate to the source repository's build scripts.

GitHub Actions synchronizes the same manifest independently before its Linux agent and Windows builds. Both receive the same normalized Cargo/agent version. For a manual build, run:

```text
python -X utf8 tools/version.py
python -X utf8 tools/version.py --check
```

The first command updates Cargo's workspace version and local workspace entries in Cargo.lock, Flutter's pubspec version, and the generated `lib/app_version.dart`. The second command only validates. Registry dependency versions remain unchanged. Never edit the generated Dart constants to change the release.

| Manifest example | Used for |
|---|---|
| `0.2` | Visible app version and installer/portable package names |
| `0.2.0` | Cargo crates and agent version verification |
| `0.2.0+1` | Flutter app version plus build number |
| `0.2.0.1` | Windows installer version information |

Git revision and source fingerprints remain in `build-info.json` and build logs to identify the exact source used. Package names use the chosen release version. Local portable folders and ZIPs also keep a timestamp to avoid overwriting previous packages.

The kit discovers Python on PATH, through the Python launcher/registry, or a standard per-user installation. Set `AETHERDESK_PYTHON` to a Python executable for a custom location. No specific Python install directory is required.

Run `python -X utf8 tools/test_version.py` and `tools/build-kit/Test-Release-Version.ps1` to check version validation and synchronization using isolated temporary fixtures.
