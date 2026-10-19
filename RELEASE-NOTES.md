AetherDesk 0.1.2 build 1 — initial Rust preview

- Optional automatic update checks for installer and portable distributions; no GitHub account or extra runtime installation is needed.
- Signed update metadata and package verification, download progress and confirmed restart.
- Connections sidebar name/status alignment.

Validation for this build:

- All seven build stages passed, including Windows/Linux compiler regressions and bundled X11 tests.
- All four updater regressions passed.
- All 7,158 files in the portable archive match the verified package.
- The extracted portable app passed its isolated dark/light rendering and editor-popup lifecycle check with the SDK removed from PATH.

This is a preview candidate. A real upgrade between two released versions, current installer upgrade/recovery, and broader live feature acceptance remain pending. Update metadata is Ed25519-signed; Windows Authenticode publisher signing is not configured.

Download the installer for a normal installation, or extract the portable ZIP and run AetherDesk.exe. Existing versions without the updater require one manual upgrade. Preview releases are not delivered through the stable automatic-update channel.
