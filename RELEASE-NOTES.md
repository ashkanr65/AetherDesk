# AetherDesk 0.1.3 — build 4

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
