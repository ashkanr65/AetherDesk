# AetherDesk 0.1.3 - build 8

Build 8 improves AI Assistant workspace and model selection, tool execution and SFTP completion reports. Full package and live provider acceptance remain required before publication.

## Changes in build 8

- **Stable AI selection:** keep the chat's model, provider, draft and explicit workspace when switching connection tabs. Browsing or editing another service no longer clears the chat's model. Preserve canonical local-workspace identity across restart.
- **Workspace control:** choose this Windows computer or any connected host from the computer/server icon beside the model selector. The choice remains fixed across tab changes; changing it revokes previous workspace approvals. Unavailable hosts never silently redirect commands to Windows. The model receives explicit target and shell instructions.
- **Provider groups:** group models beneath their service names, indent model rows and show provider/model in the selected control. Identical model IDs from different services retain separate identities. Existing search and pricing filters remain available.
- **Aligned controls:** keep screenshot, workspace, model and send/stop controls in one row. Use four equal-size approval icons with descriptive tooltips and accessible names, including Deny, without wrapping long labels.
- **Tool execution and answers:** preserve exact Windows PowerShell variables, quotes and Unicode through encoded script transport, retaining the approval gate and process cleanup. Collapse readable tool evidence instead of displaying raw JSON and empty assistant turns. Stop repeating identical failed actions, request a final explanation after repeated failures or the round limit, and shorten only the model's copy of oversized tool evidence. Full tool details remain in chat. Provider errors and empty final responses remain visible; these changes do not bypass provider credits, model limitations or permissions.
- **SFTP completion notifications:** remove successful transfer reports from the file-list footer and show a small five-second notification inside the SFTP window. Keep active progress and failed/paused transfer recovery controls. Remove only completed transfers' private resume manifests; preserve published files. Old success reports do not return on restart.
- **Chat text selection:** preserve character anchors when selecting across messages and scrolling. Allow the wheel during a held left-button selection, scroll faster as the pointer moves farther above/below the chat area, and stop edge scrolling on release or focus loss. Copy Unicode and line breaks across multiple pages. Expanded tool details share the chat's scrolling surface.
- **X11 packaging downloads:** prefer verified existing caches, retry interrupted HTTPS downloads up to four times with timeouts, and publish cache entries only after their pinned SHA-256 matches. Keep certificate verification and reject invalid cache entries; reuse the older local X11 cache when its contents match the same pinned artifacts.

## Previous changes in build 7

Build 7 improved connection startup, SSH terminal responsiveness and X11 painting checks, preserved startup diagnostics and introduced connection-scoped host cleanup.

- **Live minimized independent windows:** keep an explicitly viewed application rendering in a private capture holder when its original desktop window is minimized. Preserve real modified clicks and menu input, return it to the host window manager when viewing stops, and use the X11 server save-set to protect it if its viewing helper disappears. Real Cadence dialog families still need acceptance testing.
- **Reliability diagnostics:** probe visible GUI windows, record bounded health checkpoints and collect metadata-only Windows wait chains after repeated failed probes. Do not collect process memory or user text. The reported idle hang's root cause remains unconfirmed.
- **Safer maintenance and editor reconnect:** repair permissions on owned legacy AetherDesk maintenance directories, distinguish storage/permission/unsafe-state failures, and allow an open editor to reconnect only to the original authenticated host/account and unchanged canonical file revision. Saving remains explicit.
- **AI response limits:** configure Maximum response time in AI Assistant settings for each service; the default is ten minutes. Separate connection and streaming-idle handling, preserve partial output and do not replay timed-out requests. Keep the new option in service settings so older clients can still read application preferences after rollback.
- **Update recovery:** new helpers acknowledge startup health after multiple frames and a 20-second grace period, while retaining compatibility with older helpers. Perform acknowledgement off the interface thread and retain the latest completed recovery set for 24 hours. Closing normally during the grace period defers confirmation to the next launch instead of rolling back a successful installation.
- **Build provenance:** embed the release build in Windows file-version metadata, include the source fingerprint in build-kit startup logs, and include the UI dependency lock in the source. Add read-only regression workflows and public security/contribution guidance; publication remains manual.

- **Independent application responsiveness:** negotiate event-driven image updates with independent ordered input, bounded image backpressure, foreground priority, damage-region capture, reusable menu metadata and partial Windows texture uploads. Preserve lossless colours and complete updates across skipped UI paints. Retained older helpers have a clean legacy fallback.
- **Source application titles:** use the real application/document title and follow renames. Private native bindings keep duplicate captions separate for keyboard focus, menus and monitor placement.

- **Cadence application menus:** initialize the host's Wayland pointer focus before independent-window input. This restores toolbar and dropdown hit testing in application-only sessions, including retained sessions started with an older agent, without injecting mouse clicks or modifier keys.

- **Terminal selection across history:** keep the text anchor while scrolling with the wheel or dragging above/below the terminal. Edge scrolling accelerates with pointer distance and stops on button release/focus loss. Copy selections spanning multiple screens with correct Unicode, blank lines and soft wrapping; preserve terminal-application mouse reporting and Shift-drag override.

- **Terminal folder synchronization:** wait for a private Bash prompt confirmation before sending a folder change. Typing/pasting cancels unsent changes, busy foreground commands receive no navigation input, and an unconfirmed helper stops further synchronization until reconnect. Preserve genuine command output/errors and user command history. Support Bash 4.4 prompt hooks as well as newer Bash arrays, including existing comments and repeated initialization.

- **Moving between monitors:** coalesce native Windows DPI changes until the title-bar drag finishes, then apply the destination DPI once. This shared handling covers the main window, files/editor windows and independent application views, avoiding competing framework resizes while straddling monitors.

- **SSH+X11 painting:** retain the Composite window backing required for visible application/dialog content, disable only optional Windows per-pixel alpha, and preserve software OpenGL and normal TrueColor rendering. Packaging now checks actual native-window pixels as well as off-screen colours, overlapping copies and repeated movement/resizing. The earlier VcXsrv crash's exact trigger remains unconfirmed; these changes are not a proven source-level fix for that crash.
- **SSH terminal responsiveness:** notify the client when shell output arrives instead of waiting for the 10 ms output-polling tick. Apply bounded UI batches and output backpressure, preserving byte order and terminal closure. Avoid drawing default-background rectangles or text layouts for empty cells while retaining ANSI backgrounds, underlines, selection and cursor rendering. These changes do not establish an end-to-end speed advantage over another terminal application.
- **End remote session:** use the theme's red error colour for the power icon in both themes, with a subtle red hover background/outline across tab, fullscreen and all-monitor controls. Disconnect retains its normal icon treatment.
- **Remote-desktop handshake:** preserve early cursor, clipboard and display notifications while waiting for the required startup response. Replay them to the normal consumer after startup instead of rejecting the connection or discarding the notifications. Buffering is bounded by count, size and the handshake deadline. Unexpected control responses, session identity mismatches and incompatible protocol versions still fail explicitly.
- **Startup failure evidence:** capture a bounded server-log snapshot before rolling back a newly created desktop. Retained desktops remain resumable. Unexpected responses record the handshake phase and message type without logging nonces, endpoint tokens or clipboard contents.
- **Host cleanup for all connection types:** SSH, SSH+X11, SFTP and remote-desktop connections receive independent private temporary folders on Linux hosts with Python 3. Closing the connection, losing its SSH channel or receiving HUP removes only that connection's owned temporary files. Later connections retry abandoned, unlocked folders.
- **Preserved work:** disconnecting a remote desktop keeps its applications and runtime. Ending it first stops the desktop, then removes its session files. Cleanup preserves other tabs, unrecognized files, mounted paths and resumable SFTP transfer data.
- **Shared agent-cache retention:** retain the current version, the two newest versions and versions referenced by live processes or connection leases. Remove only explicitly marked, manifest-checked, unused caches older than seven days. If process ownership/use cannot be established, preserve the cache.
- **Bounded host logs:** limit each desktop's agent log to 4 MiB plus two rotations, including trimming oversized legacy logs when the new watcher takes ownership. Keep draining worker output if a log write fails, so a full disk does not block the desktop's output pipe.
- **Diagnostic history:** retain at most twenty small session diagnostic archives for seven days, separately from removed runtime folders. Failed cleanup remains eligible for a later safe retry; unknown legacy state is preserved.

## Remote Applications and X11 forwarding

- **Independent application views:** follow per-monitor Windows display scaling, add a separate application zoom control, and keep menus within the local monitor work area. Moving between monitors does not request a server application resize. Recover transient application focus changes silently while preserving input-release safeguards.

- Add **Remote Applications** as a separate connection type: a terminal and searchable application launcher, with up to eight independent host-rendered views. New top-level windows open automatically; closing a view keeps its application running.
- Use a private graphical session with a lightweight host window manager, inherited startup exports/cwd and D-Bus environment. Keep applications launched through the panel on disconnect; explicit End session stops the launcher process group. Desktop and application profiles use separate retained/recovery identities.
- Reuse existing lossless window/menu capture, monitor/DPI placement, cursor shapes, modifier release, AltGr/keypad/wheel handling, clipboard worker, SFTP/editor transfers, monitoring and logs. Do not copy, encode or transmit an unused full desktop image in this mode; preserve normal repaint callbacks.
- Route independent-window image helpers through the dedicated graphics SSH transport, keeping terminal/control traffic independent of large window images. Both links use the same connection-scoped authentication and VPN.
- Batch already-queued independent-window input in bounded, ordered packets, preserving modifier and release order without waiting to fill a batch. Prevent failed helper retries from launching a new SSH process every UI frame.
- Optimize SSH+X11 forwarding with one authenticated setup write and asymmetric copy buffers for larger redraws and prompt small replies. Preserve cookie validation, lossless compression, separate terminal/graphics connections, half-close behaviour, cancellation and Cadence's pixel colours. These changes do not remove X11's inherent synchronous network round trips.

## Validation and deployment

Native regressions cover reordered startup notifications, bounded buffering, strict control/session validation, evidence captured before rollback, cleanup isolation, abandoned leases, HUP, cache retention, log rotation and failed log writes. Complete Build 7 packaging and live installation/update acceptance must be completed before publishing. These changes do not by themselves establish that every cause of intermittent handshake failure has been reproduced or eliminated.

The host-maintenance helper is optional and requires Python 3 on Linux. Without it, connections continue and maintenance is reported as unavailable. Existing retained desktops continue using their running host agent; save remote work and end/start that desktop to use the new host-side code. Updating or disconnecting the Windows client does not itself end retained applications.

## Earlier release notes (historical)

## AetherDesk 0.1.3 - build 6

AetherDesk 0.1.3 build 6 improves remote-desktop rendering, keyboard and wheel handling, SFTP transfers and connection setup.

## Changes in build 6

- **Remote-desktop performance:** share unchanged image tiles between capture, decoding and display, and update only changed texture regions. Changes are compared with the last delivered or displayed image so skipped frames do not lose earlier updates. Lossless colour data is preserved; these changes do not introduce lossy image compression or establish a particular speed improvement on every system.
- **Host responsiveness:** wake the compositor for incoming commands, remove a redundant lossless frame-rate delay, and reduce repeated process discovery for terminal flow control while continuing to revalidate known terminals.
- **Keyboard and wheel handling:** avoid duplicate key presses caused by overlapping repeat handling, preserve the order of physical AltGr and ISO keyboard events, and improve routing for discrete and high-resolution wheel input in X11 applications inside the remote desktop. The client and host keyboard layouts must still be configured appropriately.
- **Connection startup commands:** optionally run saved Bash commands on the selected connection, with a timeout and a configurable failure policy. Successful exports and the working directory carry into the new session. Retained remote desktops run startup commands only when first created. Startup commands are saved in the connection profile; do not put passwords or API keys in them.
- **SFTP transfers:** restore drag-and-drop between local and server panes, show persistent byte/file progress and queue errors, and retain verified partial data for resume. Transfer action buttons have consistent sizes. Queue progress remains compatible with older saved transfer records.
- **Folder downloads:** copy ordinary files and folders while skipping and reporting symbolic links, including links to targets outside the selected folder. Link targets are not copied. Unsupported filenames and special files are still reported rather than guessed or silently rewritten.
- **File navigation:** each pane has its own refresh control, with a circular-arrow icon. Refreshing one pane does not navigate or refresh the other pane. Go, parent-folder and new-folder controls use icons.
- **Connection controls:** keep the toolbar compact above the remote desktop or SSH terminal, with disconnect and end-session controls on the right. Remove the duplicate remote display-settings icon and keep monitor information under the resolution control.
- **Connection form:** align WireGuard path, Browse and Remove controls on one row, with its enable option below. Align the startup timeout field and place the actions at the bottom right in the order Cancel, Save connection, Save and open.
- **Interface details:** distinguish Tools from AI Assistant, use consistent Ready indicators, and retain Bookmarks as a quick-access list for favourite saved connections.
- **Diagnostics and regression coverage:** record retained-agent exit evidence and strengthen checks for frame preservation, input routing, transfer progress, pane refresh and layout bounds. Terminal-cache tests wait for observable child processes and use a controlled clock to avoid build-machine timing races.

## Validation and known limitations

All seven stages of the completed Windows x64 build passed, including Windows/Linux regression checks, bundled X11 verification, portable packaging and installer compilation. The source fingerprint and package version are verified separately before publication. These build results do not establish a real installed-client upgrade/restart test or complete EDA application acceptance.

Independent application windows remain experimental. Application menus, mixed-DPI movement, clipboard integration and application-specific input still require wider live validation. The reported intermittent idle hangs and server-stream disconnections are not claimed to be completely resolved.

Existing retained remote desktops keep their running Linux agent. To use the new host-side changes, save remote work, end the old remote session and start a new one; disconnecting alone preserves the existing session and agent.

## Downloads and updates

Use the Windows x64 installer or extract the complete portable ZIP. Existing users can open **Updates** and choose **Check now** after this release is published. Update packages are checked against signed metadata. Save work before installing; updating the Windows client does not itself close remote applications.

## Earlier release notes (historical)

## AetherDesk 0.1.3 - build 5

AetherDesk 0.1.3 build 5 adds installation of downloaded updates after normal close, stronger SSH+X11 failure isolation, and recovery of Windows crash evidence at the next startup.

## Changes in build 5

- **Updates after close:** a new Updates preference installs a fully downloaded, verified package after a normal safe exit. AetherDesk stays closed; its next launch uses the new version. Automatic downloading remains off by default, and Update and restart remains available. Windows shutdown and incomplete/busy operations defer installation.
- **Recovery safeguards:** wait for other windows using the same installation to close; retain the previous application files until the new version renders successfully. Installed packages also preserve application/uninstaller files for recovery, though restoring installer registry state may require installer repair.
- **SSH+X11 resilience:** a failed local X server no longer disconnects the terminal or SFTP. Preserve its diagnostic tail and offer a Restart X11 icon using the original forwarding identity. Applications lost with the failed X server must be reopened.
- **Crash evidence:** silently check recent Windows Application Error, Application Hang and correlated Windows Error Reporting events in the background at startup. Matching AetherDesk and bundled X11 reports appear in Logs, with duplicate reports suppressed and unrelated payloads excluded. Windows may not record every failure.
- **Copy and paste:** preserve clipboard shortcut presses on the GUI fallback path, including Ctrl+Shift variants, empty/non-text Windows clipboards and independent application views. Keep native Ctrl/Shift capture rules and foreground-window scoping. Check the Windows clipboard immediately when entering the remote surface.
- **Display changes:** wait briefly for a stable display layout before starting X11 and explicitly use its existing ShadowGDI path. This does not establish the root cause of the reported X11 exit or guarantee that it cannot recur.

Windows regression and isolated bundled-server results are recorded in SESSION-UI-TASKS.md. A complete build 5 package and real installed/portable two-version update acceptance are still required before publication. No release has been published for these changes.

## Earlier releases

## AetherDesk 0.1.3 - build 4

AetherDesk 0.1.3 build 4 fixes reconnection to failed saved desktops and adds a standalone publishing utility for maintainers.

## Changes in build 4

- **Remote desktop recovery:** opening one saved connection can recover a confirmed failed retained desktop instead of requiring a second tab. Healthy disconnected desktops and their applications remain resumable.
- **Host-agent lifecycle:** publish connection details after desktop startup, detect fatal failures while connected or detached, and clean up failed brokers. Serialize concurrent launches for the same desktop.
- **Recovery safeguards:** use the existing authenticated session-close command, require confirmed cleanup before replacement, and preserve sessions when their health cannot be established. Include compatibility with older retained agents.
- **Publisher utility:** provide a standalone Python environment and a double-click launcher for completed-build verification, signing and release publication. This is a maintainer tool; ordinary AetherDesk users do not need Python.

The affected agent/client lifecycle packages passed 45 native tests. Isolated real-process checks verified healthy legacy reuse, recovery from a failed older broker in one launch, and cleanup after a new compositor failure. Six recovery checks are registered in the build kit. The complete build 4 Windows/Linux packaging run and real installed-client upgrade acceptance remain pending.

## Previous releases

AetherDesk 0.1.3 build 3 adds connection-scoped server monitoring and configurable licence/custom checks.

## Changes in build 3

- **Monitoring:** vertically stacked CPU, memory/swap, recorded-login, storage, network-interface, uptime and connection-response cards, with an always-visible icon status bar, per-item visibility, compact account/session counts and an Add monitor popup.
- **Licence usage:** support lmutil and standalone lmstat, including multi-seat checkouts; add a FlexNet/Cadence card; hide unused features and filter by feature, user or machine. Add friendly feature labels, group results by user and open full details separately.
- **Custom checks:** add a server-side command with Text, Single value or JSON table output; test it, save it, enable/disable it and filter its results.
- Checks run in the background on the selected tab's existing SSH/VPN connection, outside the interactive terminal. Sources share results; timeouts, cancellation and stale/disconnected states are bounded.
- **Home storage:** show the home filesystem and offer a bounded manual scan of home folders, with unknown/incomplete sizes clearly identified.
- **SSH terminal history:** add a right-side scrollbar that stays separate from remote application mouse input.
- See MONITORING.md for configuration, output formats and limitations. Build 3 passed Windows/Linux compiler regressions and bundled X11 verification. Live monitoring and upgrade acceptance remain separate.

## Changes in build 2

- **Ctrl and Shift:** balance modifier releases on Windows, repair missed modifier edges before remote mouse selections, and avoid duplicate modifier injection during native/GUI capture changes. Brief capture-lock contention no longer falsely releases host input.
- **Mouse wheel:** use one GUI wheel route, activate the remote image before scrolling, and preserve small high-resolution wheel movements without amplifying them. Independent views restore their target focus before sending scroll input.
- **Remote cursor shapes:** forward host cursor images so resize and text pointers can appear over the remote desktop.
- **Independent application windows:** improve owned-menu focus handling and mixed-DPI resize stability. Fix a nested GUI-context lock that could deadlock independent-view sizing.
- **Appearance:** preserve saved light/dark palettes and reset defaults more reliably, including when another application instance saves unrelated preferences.
- **Reliability and diagnostics:** avoid awaiting completed shutdown tasks twice and add bounded background evidence when the Windows GUI stops responding. Installation shortcuts use a consistent working directory.

All seven build stages for the published build 3 passed in 31 minutes 42 seconds. Those results apply to build 3; live monitoring, clean-machine installation, real upgrade/restart and wider EDA acceptance are not established by those build results.

## New and improved

- **Independent remote application windows (experimental):** choose an already-open X11 application from a remote desktop and view it in its own Windows window. The application stays in its original server session. Closing the independent view keeps the application running.
- Independent views open at the original application's physical pixel size. Manual resizing requests an application resize, and owned dropdown menus and submenus can extend beyond the main application edge.
- Lossless application and menu capture preserves RGB values. The feature uses the existing authenticated, per-session SSH/VPN connection and adds no public network listener.
- **Keyboard improvements:** dedicated numpad operator routing, corrected duplicate modifier injection, and reconciliation of stale Shift state in GUI fallback. Native capture recovery now backs off instead of rapidly restarting.
- **Host status clock:** shows the selected server's time and changes to `--:--:--` after disconnection or three seconds without a fresh response.
- **SFTP resilience:** a non-UTF-8 filename no longer breaks the entire directory listing. Unsupported names remain visible but disabled; other files remain accessible. File operations never guess a replacement path.
- **Idle/wake handling:** native clipboard access runs outside the GUI thread, heartbeat backlog is skipped after sleep, and heartbeat writes have a timeout.
- Fullscreen/all-monitor transitions have safer native placement, texture-resize handling and graphics-limit checks.
- Desktop failures retain more useful channel and bounded server-agent diagnostic evidence.

## Known limitations

Independent application windows are experimental and have known bugs. Keeping an independent view live after minimizing the original application is not yet supported. Automatic managed-dialog families, multiple independent views per connection, complete IME/clipboard integration, true removal of the original desktop window, and reconnect restoration remain incomplete. Actual EDA application menus, colours and mixed-DPI behavior still require wider validation. Remote desktop and SSH+X11 remain available alongside this feature.

Files with non-UTF-8 names cannot yet be opened or transferred through AetherDesk. Recursive operations involving them are refused rather than silently omitting files or changing their names.

The reported idle hang and intermittent server-stream disconnections have not been reproduced to a confirmed root cause. This release includes targeted fixes and improved diagnostics; complete resolution is not claimed. During native-capture recovery, Windows may handle some system shortcuts locally.

## Downloads and updates

Use the Windows x64 installer or extract the complete portable ZIP. Existing users can open the gear menu, choose **Updates**, and select **Check now**. Save work and disconnect sessions before installing an update. AetherDesk does not close your remote applications as part of updating the Windows client.

Windows and Linux compiler regressions and bundled-X11 verification passed for build 3. The complete build 4 packaging run remains pending. Update packages are verified against signed metadata. Clean-machine, real upgrade/restart and comprehensive live detached-window acceptance remain pending.
