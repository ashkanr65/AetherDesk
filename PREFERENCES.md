# AetherDesk Preferences

Open the top-right gear menu and choose **Preferences**. These controls belong
to AetherDesk, not the remote host's monitor resolution or streamed pixels.

| Category | Controls |
| --- | --- |
| General | Start maximized, reopen saved tabs disconnected, confirm quitting active connections. Unsaved editors retain their own protection. |
| Display | System/light/dark mode, application scale and font size, separate light/dark palettes and sample GUI previews. |
| Connections | Default SSH port, remember-username default and future connection-attempt timeout. Existing saved profiles are unchanged. Reconnection remains explicit. |
| Files and Editor | Directory synchronization default for new SSH tabs and built-in versus Windows-default file opening. Explicit per-file actions take priority. |
| AI Assistant | Open existing service/model/workspace/tool-permission preferences. Keys remain protected separately. |
| Keyboard | Change Ctrl-based application navigation shortcuts. Remote keyboard and terminal input handling remain separate. |
| Updates | Open existing update controls, including automatic checking/downloading, release notes and confirmed installation. |
| Logs | Capture level, size per file, retained rotations per application instance and the Log files viewer. Capture level changes apply after restart. |

Categories remain on the left and content scrolls on the right. **Apply and save**
validates and saves changes. **Cancel** discards unapplied changes. **Reset to
defaults** updates the draft; apply it to save. Separate AI and update management
windows retain their own save and confirmation behaviour.

## Theme colours

Choose **Display**, then **Light colours** or **Dark colours**. Both palettes are
saved independently. The miniature workspace uses sample content, never a
screenshot of a real connection. Use a colour picker or a six-digit hexadecimal
value such as `#C7A2B3`.

Roles cover workspace/panels, toolbar, sidebar, icon rail, dialogs, inputs, title
bars, tabs, text, icons, borders, separators, buttons, hover/pressed/selection
states and status colours. Low-contrast combinations warn but remain allowed.
Invalid hexadecimal values must be corrected before applying.

**Reset this palette** restores only the selected palette in the draft. Theme
imports update the draft and require Apply and save. Exports write both draft
palettes to a new JSON filename; existing files are never overwritten. Enter
the full file path in the theme-file field before importing or exporting.

Theme exports contain only schema and palettes, without saved connections,
passwords, API keys, workspaces or chats. Terminal font and syntax settings
remain available through the terminal settings icon.

## Compatibility and recovery

The existing `%APPDATA%/AetherDesk-Rust` data location is retained. New settings
use `preferences.json`; sidebar layout still uses `settings.json`. Directory
synchronization falls back to the previous terminal preference until explicitly
set in Preferences. Secrets and updater configuration retain their stores.

Missing preferences use compatible defaults. Damaged or unsupported files are
preserved and cannot be overwritten through the dialog. Repair or move such a
file while AetherDesk is closed to restore saving. Imports are bounded and reject
invalid data. Startup restoration stores saved-profile IDs only and never
connects automatically.

Registered regressions cover persistence, independent palettes, contrast,
imports/exports, navigation and log-window bounds. Compiler and packaged Windows
verification run when the user builds; formatting and offline recipe checks
do not substitute for those tests.
