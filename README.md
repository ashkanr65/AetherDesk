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

## Connection startup and transfers

Edit a saved connection and enable **Run startup commands** to prepare that
host with a Bash script. The editor accepts multiple lines; Enter adds a line.
Choose a timeout and whether connection setup should continue after failure.
Exported variables and the working directory carry into the terminal or new
remote desktop. Reconnecting to a retained desktop does not run the script
again. Commands are saved in the connection profile; imported automation stays
disabled until explicitly enabled on this installation.

File and folder uploads and downloads show progress below the SFTP panes,
including transferred size, percentage and folder file counts. Scanning and
verification remain visible, and Pause, Resume and Cancel stay available.
Disconnect and End remote session are grouped on the right of the connection
toolbar, separately from file, terminal and display controls.

If the Linux desktop agent exits unexpectedly, its independent supervisor
records the exit code or signal in the private session's `agent-exit` file and
invalidates stale ready status. Client failure diagnostics collect that record
alongside the bounded agent log; this cannot recover unsaved work from a crashed
host application.

## Remote desktop performance

The lossless desktop path preserves exact BGRA colours. Captures, decoded
surfaces and prepared textures share unchanged tiles, so small updates do not
copy or convert an entire multi-monitor desktop. Each snapshot remains complete:
skipped captures or GUI updates retain earlier changes. Resizing, reconnecting
and texture recreation still rebuild the required image safely.

Capture controls the frame rate and reacts to command wakeups; transmission
uses delivery credits instead of adding a second frame-rate wait. The terminal
flow-control worker caches discovery while continuing to validate active PTYs
and prevent Ctrl+S output freezing. These are local processing changes and do
not require a different wire protocol or lossy compression.

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
