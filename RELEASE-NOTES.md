AetherDesk 0.1.2 build 2 — update acceptance candidate

- Increases the build number from 1 to 2 to exercise update detection, download, replacement and restart from the existing 0.1.2 build 1 application.
- Application behavior is unchanged; this is a release-metadata update, not a feature release.

Validation status:

- Build 1 passed all seven build stages, updater regressions, portable integrity verification and isolated rendering/editor checks.
- Build 2 compilation, package verification and real upgrades remain pending. Build 1 results do not certify this new package.

This candidate must be built and verified before publication. A real upgrade between versions, current installer upgrade/recovery, and broader live feature acceptance remain pending. The publisher signs update metadata with Ed25519; Windows Authenticode publisher signing is not configured.

Keep the existing build 1 installation for the upgrade test. A draft or preview cannot trigger its stable update channel; publishing a normal release is a separate step and makes the update available to all eligible users.
