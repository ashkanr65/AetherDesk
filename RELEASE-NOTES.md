AetherDesk 0.1.2 build 3 — Preferences and theme customization

- Preferences replaces the application Settings page, with General, Display, Connections, Files and Editor, AI Assistant, Keyboard, Updates and Logs categories.
- Independent light and dark palettes provide 30 editable colour roles, sample GUI previews, hexadecimal input, contrast warnings, reset and theme import/export.
- Application font size and scaling, system theme, startup window state, reopening disconnected tabs and confirmation before quitting active connections.
- New-connection defaults, connection timeout, file-opening defaults, directory synchronization and customizable application navigation shortcuts.
- Configurable log level, log size and rotation retention; the Log files popup keeps stable bounds while scrolling.
- Public interface, About, package names and installer branding use AetherDesk. Existing settings, protected credentials, installation identity and update verification remain compatible.

Validation status:

- Rust formatting/parser and offline build-recipe checks are performed without compiling the application.
- Seven new native regression checks are registered for the user's build-kit test stage. They have not been compiled or run by the assistant.
- Source and releases are consolidated in public ashkanr65/AetherDesk; MIT OR Apache-2.0 licence files are included.
- Packaged UI checks, real upgrades, restart/recovery and data preservation remain pending.

This release must be built, tested and signed before publication. No new binary is published by these source changes. Update metadata uses the existing Ed25519 key; Windows Authenticode publisher signing remains separate.
