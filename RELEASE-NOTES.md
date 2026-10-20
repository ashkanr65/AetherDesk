# AetherDesk 0.1.2 build 6

AetherDesk brings remote Linux work into one Windows workspace: terminals,
files, graphical applications, desktops and an optional AI assistant. Open
connections in independent tabs and keep the tools for each task together.

This release introduces AetherDesk to the public as a free, open-source project.
Source code, downloads, issues and future releases are available in the same
repository: https://github.com/ashkanr65/AetherDesk.

## Connections and remote work

- **SSH terminals:** interactive shells, saved connections, terminal appearance
  and word-colour customization, host-key verification and keyboard shortcuts.
- **SSH + X11:** run individual Linux graphical applications using the bundled
  Windows X11 runtime.
- **Linux remote desktops:** work in a tab, on one fullscreen monitor or across
  monitors. Presentation controls include resolution selection, file transfer,
  disconnect and ending a remote session.
- **Per-connection WireGuard:** browse for a configuration and enable or remove
  it for the individual connection, with isolation between tabs.
- **Saved sessions:** search and reopen connection profiles, optionally remember
  usernames and save passwords or key passphrases in protected Windows storage.
  Optional master-password protection is available for saved secrets.

## Files and editing

- Browse local and remote folders and transfer files through SFTP in a separate
  window, without shrinking the terminal or desktop.
- Choose whether the SSH terminal and file browser follow each other's folder.
- Use the built-in text editor or the Windows default application. Working-copy
  recovery, comparison and explicit replacement confirmations help protect work.

## AI assistance

- Connect OpenRouter, NVIDIA or a custom OpenAI-compatible service using your
  own account and API key. Keep multiple services and their models separately.
- Search model lists, filter available pricing information and select models
  from the chat composer. Start a fresh chat or return to conversation history.
- Attach a connection screenshot and choose a local or remote workspace. Tool
  requests can work with files, commands and connections subject to permissions.
- Configure context/output limits and supported reasoning options. Availability,
  capabilities, pricing and free quotas are determined by the provider; free
  AetherDesk software does not include paid AI service credits.

## Your workspace

- Use light, dark or system appearance, application scaling and font-size controls.
- Customize the light and dark palettes independently with colour pickers,
  hexadecimal values, a sample workspace preview and palette-only import/export.
- Preferences groups General, Display, Connections, Files and Editor, AI
  Assistant, Keyboard, Updates and Logs in one place. Each editable section has
  its own Apply, Reset and Cancel actions, always visible below its content.
- Resizable sidebars, compact layouts, tab controls and application shortcuts
  adapt the workspace to different window and monitor sizes.
- View, filter and export logs to help investigate connection or application issues.

## Download and get started

**Supported client:** Windows 10/11 x64 with an OpenGL 3.3-capable graphics driver.
A remote connection also requires a reachable host and suitable credentials.
Linux desktop and graphical application features require a compatible Linux host.

1. Download the **Setup.exe** installer or the **windows-x64.zip** portable package
   attached to this release. The automatically generated GitHub source archives
   are source code, not the ready-to-run Windows application.
2. Run the installer, or extract the entire portable ZIP to a writable folder
   such as **AetherDesk**. Keep all bundled files beside the executable.
3. Start **AetherDesk.exe**, choose **New Connection** and enter your host details.
4. Use the top-right gear menu for Preferences, Updates, Log files, Help and About.
   Configure an AI service only if you want to use the assistant.

Client runtimes are bundled. Running the Windows package does not require
installing a compiler, Python, Node, GStreamer or a separate X server.

## Updates, data and security

AetherDesk checks update metadata signed with the project's established Ed25519
key and verifies downloaded package hashes. Update preferences control automatic
checks and downloads; installation asks before restarting. Draft and preview
releases are excluded from stable updates.

Saved passwords and API keys are kept separately from session profiles in
protected Windows credential storage. Palette exports contain colours only.
AI requests and attachments are sent to the configured provider; choose services,
workspaces and tool permissions appropriate for your data.

Portable means no installer is required; application preferences and protected
credentials still use the Windows user's existing data stores. Those locations
and the installer identity remain compatible with earlier builds.

## Upgrade notes and current limits

- This package includes repairs for portable updates involving read-only bundled
  files, safer file replacement and recovery, and responsive application layouts.
  Earlier clients stuck in recovery may need a manual upgrade or repair before
  receiving these fixes automatically. Keep their recovery folders until resolved.
- The welcome page uses compact content-sized spacing. Preferences keeps its
  navigation and section actions outside the scrolling content, including the
  Display palette editor.
- Updates use signed metadata; Windows Authenticode signing is not configured.
- Host environments, mixed-DPI monitors, graphics drivers and AI providers vary.
  Build checks do not establish compatibility with every EDA application or prove
  clean-machine installation and real upgrade/restart acceptance. In particular,
  confirm colour fidelity with your own graphical applications and workflow.

## Open source and feedback

Original project code is dual-licensed under **MIT or Apache 2.0**, at your option.
Bundled dependencies retain their own licences; packages include third-party
notices and supplied licence files.

Read the README for setup and the release guide for building and publishing.
Report reproducible problems at https://github.com/ashkanr65/AetherDesk/issues,
including the application version, Windows version, connection type and relevant
redacted logs. Do not include passwords, API keys or private WireGuard keys.
