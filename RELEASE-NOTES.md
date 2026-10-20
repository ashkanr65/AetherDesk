AetherDesk 0.1.2 build 3 — Preferences and theme customization

- Preferences replaces the application Settings page, with General, Display, Connections, Files and Editor, AI Assistant, Keyboard, Updates and Logs categories.
- Independent light and dark palettes provide 30 editable colour roles, sample GUI previews, hexadecimal input, contrast warnings, reset and theme import/export.
- Application font size and scaling, system theme, startup window state, reopening disconnected tabs and confirmation before quitting active connections.
- New-connection defaults, connection timeout, file-opening defaults, directory synchronization and customizable application navigation shortcuts.
- Configurable log level, log size and rotation retention; the Log files popup keeps stable bounds while scrolling.
- Public interface, About, package names and installer branding use AetherDesk. Existing settings, protected credentials, installation identity and update verification remain compatible.

Validation status:

- The user-built installer and portable package passed all seven build-kit stages in 26 minutes 29 seconds on 20 October 2026.
- Windows and Linux registered regression checks passed, including the seven new Preferences/theme/log/navigation checks and the packaged X11 test.
- Built package source fingerprint matches main's application/build inputs. Project licence files match the repository.
- The completed executable passed isolated dark/light rendering and editor popup open/close checks. No application compilation was run by the assistant.
- Complete source, releases and MIT OR Apache-2.0 project licences are available together in ashkanr65/AetherDesk.
- Real two-version installer/portable upgrades, restart/recovery, settings and credential preservation, clean-machine acceptance and broader live connection checks remain pending. This publication does not claim those tests passed.

Update metadata uses the existing Ed25519 key. Windows Authenticode publisher signing remains separate and unconfigured. Updates ask before restarting; use the gear menu → Updates → Check now to find this build.

Published as the latest stable update on 20 October 2026:
https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build3
Signed assets, remote digests and anonymous update-feed access verified.
