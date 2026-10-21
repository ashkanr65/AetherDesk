# AetherDesk

AetherDesk is a Windows workspace for SSH terminals, SFTP file transfers,
Linux desktop connections, SSH with X11 applications, editing and AI assistance.
Connections open in independent tabs, with files and editors available in
separate windows.

## Downloads

Download the installer or portable ZIP from
[AetherDesk Releases](https://github.com/ashkanr65/AetherDesk/releases).
Extract the complete portable ZIP and run `AetherDesk.exe`. Required application
runtimes are bundled; users do not install Rust, Flutter, Node, Python,
GStreamer or an X server to run the Windows client.

Windows 10/11 x64 and a Direct3D 12 capable graphics driver are required for
the default renderer. An OpenGL 3.3 compatibility option is available under
Preferences > Display; renderer changes require restarting the application.
Connections need a reachable host and suitable credentials. AI providers use
the user's service configuration and may have their own charges.

## Preferences

Open the top-right gear menu and choose **Preferences**. Categories include
General, Display, Connections, Files and Editor, AI Assistant, Keyboard, Updates
and Logs. Display controls customize AetherDesk itself; remote desktop
resolution and terminal syntax colours remain separate.

Light and dark palettes have independent colours, sample workspace previews,
colour pickers, hexadecimal values, import/export, contrast warnings and reset.
Changes stay in the preview until **Apply and save**. See
[Preferences](PREFERENCES.md) for usage and storage details.

## Build and release

Windows application source is under `ui/rust` and its backend is under
`core/rust`. Use `tools/rust-build-kit`, or the accompanying beginner build kit.
`Build-Everything.cmd` produces both installer and portable distributions;
`Build-Faster.cmd` retains caches for repeat builds. Required checks still run.

The [complete release guide](RELEASE-GUIDE.html) explains versioning, building,
package verification, signing, draft uploads, publication and real update tests.
[UPDATES.md](UPDATES.md) describes the updater and recovery limits.
The [build kit instructions](tools/rust-build-kit/README.md) list prerequisites
and output locations.

## Updates and protected data

Updates use signed metadata and verified package hashes. Select **Updates**
from the gear menu. Installation asks before restarting and checks for active
work. Drafts and previews are excluded from stable updates.
Saved passwords and API keys use the protected Windows credential store,
separately from session files. Theme exports contain palettes only.

Existing application-data directories and installer identity remain unchanged
for compatibility. Paths containing earlier implementation names are internal
compatibility details, not different products.

## Licence

Original project code is available under [MIT](LICENSE-MIT) or
[Apache 2.0](LICENSE-APACHE), at your option. Dependencies retain their own
licences; distribution packages include third-party notices and supplied licences.

## Project status

Source code, issues and releases share [ashkanr65/AetherDesk](https://github.com/ashkanr65/AetherDesk).
Development uses main. Builds and signing workflows run only when a maintainer
starts them manually; source pushes do not build or publish releases. Historical
development records remain available for engineering traceability.

Build checks and live acceptance are tracked separately. New source changes are
not validated by an older published binary's results. See
[release notes](RELEASE-NOTES.md) for the current candidate and
[remaining work](SESSION-UI-TASKS.md) for outstanding checks.
