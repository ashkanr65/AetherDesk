# Server monitoring

Open a connection, then choose **Monitoring** in the left toolbar. Cards are
stacked vertically: connection response, CPU, memory/swap, recorded logins,
home storage, network interfaces and uptime. The compact status bar is always visible,
with vector icons and vertically centered readings. Each card has a **Show in status
bar** checkbox; custom checks have the same option. The left panel remains optional.
Click a system reading to open Monitoring; click a custom reading for details. The
**+** at the right edge opens Add monitor. Narrow bars scroll horizontally without
wrapping or moving the + button. Wide result tables open in a separate details popup.

System CPU/memory/network counters refresh about every three seconds. Login and
home filesystem information refresh about every thirty seconds. CPU and network
rates need two samples. Network rates are per server interface, not AetherDesk's
own traffic. Response time includes the check's execution, not just network latency.
Recorded logins are accounts/sessions in the server's login records, not a complete
list of all GUI/background users or proof of active work. Built-in system cards
require Linux with /proc and standard system utilities.

## Add a licence monitor

1. Click **+** in the compact bar, or **+ Add monitor** below the Monitoring list.
2. Choose **Licence usage (FlexNet / Cadence)**.
3. Enter a name, the server-side lmutil or standalone lmstat executable path, and the licence server
   (`port@host`) or licence-file path. Use a full executable path when it is not on PATH.
4. Choose a refresh interval of 30–3600 seconds; the default is 60 seconds.
5. Set optional feature/tool, username and client-machine filters. Unused licence
   features are hidden by default. Enable **Include unused features** to see them.
6. Optionally add friendly labels, one `exact_feature=Display name` per line.
   Choose **Group by user** to see each account and its reported features.
7. Click **Test**, then **Save monitor**. Test does not save the card.

The check runs `lmutil lmstat -c <source> -a -t 5`, or `lmstat -c <source> -a -t 5`
when the executable basename is lmstat. Arguments are quoted as data. For the
generic example, enter `/opt/vendor/bin/lmstat` and `5280@license-server.example`.
Checkout annotations before the version are accepted. A row ending in
`, 4 licenses` represents four seats, rather than four accounts. Numeric feature
names stay literal unless you supply a friendly alias; product names are not guessed.
Feature counts remain the licence server's totals even when filtering individual
users or machines. A checkout means a licence is held; it does not establish
active application use. Reports are limited to what the licence server exposes;
technical feature identifiers do not automatically identify commercial products.
User details can be absent even when a feature has checkouts.

## Add a custom check

Choose **Custom command or script**, enter a finite server-side shell command,
and select its output format:

- **Text:** UTF-8 lines, optionally filtered by a case-insensitive search.
- **Single value:** one line of at most 120 characters.
- **JSON table:** string column names and string cells, for example:

```json
{"columns":["Tool","User"],"rows":[["Feature A","user1"]]}
```

Tables allow 1–12 columns and at most 256 rows. Output is limited to 256 KiB;
text lines allow at most 2048 characters and displays retain 32,768 characters
with a truncation notice when needed. Large previews have a details
popup; text details display at most 512 lines. Custom intervals are 10–3600 seconds.
User-provided commands run as the connected account and can change files: use
read-only checks. Saving an enabled card authorizes periodic execution while its
Monitoring panel or its details popup is visible, or **Show in status bar** is enabled.
Test runs once without saving it.

## Behaviour and isolation

- Checks use dedicated, non-PTY channels on the selected tab's existing SSH/VPN
  connection. They do not type into the terminal, enter its history, ask for sudo,
  release licences or terminate other applications.
- Cards belong to a saved connection. Results are scoped to its actual live SSH
  connection; switching connections clears old readings and cancels pending work.
- Identical sources on the selected connection share a result; filtering does not
  execute more commands. At most two requests are scheduled concurrently.
- All commands have a 12-second deadline and owned-process cleanup on cancellation,
  timeout or disconnect. Commands must not launch persistent background services.
  The existing owned-command runner requires Bash and util-linux `setsid` on the
  Linux SSH server; AetherDesk does not install them automatically.
- Built-in sampling stays active for the selected connected tab. Custom checks
  continue when pinned to the bar or open in Monitoring/details; otherwise they stop.
  Closing/disconnecting cancels pending checks.
- Failed or old bar readings use a warning colour and explain their state on hover.
  Disconnection clears live readings. Refresh and last-reading times live in
  tooltips, keeping the cards steady during normal periodic updates.
- Settings are saved separately in `monitoring.json` in AetherDesk's application
  data directory. A damaged settings file is preserved rather than overwritten.

## Home storage

The periodic check queries only the filesystem containing the account's home
folder, using `df -Pk -- "$HOME"`. If `/home` shares `/`, the bar correctly names
that filesystem `/`; it does not invent a separate /home volume.

**Scan home folders** explicitly scans the first 64 direct directories under
`/home`, without following directory symlinks or crossing filesystems. The whole
`du` command has an eight-second deadline and runs only with the connected
account's permissions. It is never part of the three/thirty-second sampling loop.
No sudo or quota changes are used. Folder names and owners are displayed, with
paths on hover. Cached sizes also appear in the bar when Storage is selected.
These are allocated home-folder sizes, not ownership-based disk quotas. Shared
folders, hard links, snapshots and files elsewhere can affect comparisons.
Permission failures, timeout or limits are marked incomplete: sizes become lower
bounds and missing sizes stay unknown. They are never reported as zero usage.
Run Scan again to refresh; changing live connections clears these cached results.

## Terminal history

SSH and SSH+X11 terminals have a right-side scrollbar. Its thumb follows actual
retained history, with live output at the bottom. Dragging/clicking the rail is
local; it sends no terminal application mouse events. Normal selection, wheel
reporting and application input continue inside the terminal surface. The rail
is inactive in an alternate-screen application that has no scrollback history.

Version 0.1.3 build 3 passed its Windows/Linux compiler regressions and bundled
X11 verification. Real SSH/licence behavior and UI acceptance remain separate
checks; neither offline fixtures nor compilation establishes those results.
