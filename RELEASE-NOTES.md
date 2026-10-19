AetherDesk 0.1.2 build 2 — update acceptance candidate

- Increases the build number from 1 to 2 to exercise update detection, download, replacement and restart from the existing 0.1.2 build 1 application.
- Application behavior is unchanged; this is a release-metadata update, not a feature release.

Validation status:

- Build 1 passed all seven build stages, updater regressions, portable integrity verification and isolated rendering/editor checks.
- Build 2 passed all seven user-run build stages, including Windows/Linux regressions, bundled X11 verification, portable packaging and installer creation (1:08:28 total).
- The verified package matches the current source fingerprint. Its executable passed isolated dark/light rendering and popup-editor lifecycle checks without SDK PATH or real user settings.
- All 7,158 portable ZIP entries match the signed file inventory; metadata signatures and complete asset hashes were verified.
- Real installer/portable upgrades and data-preservation checks remain pending.

This candidate has passed build and package checks. A real upgrade between versions, current installer upgrade/recovery, and broader live feature acceptance remain pending. The publisher signs update metadata with Ed25519; Windows Authenticode publisher signing is not configured.

Keep the existing build 1 installation for the upgrade test. A draft or preview cannot trigger its stable update channel; publishing a normal release is a separate step and makes the update available to all eligible users.
