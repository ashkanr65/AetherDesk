# AetherDesk 0.1.3

AetherDesk 0.1.3 adds experimental independent application windows and improves remote input, file browsing and idle handling.

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

Independent application windows are experimental and have known bugs. Automatic managed-dialog families, multiple independent views per connection, complete IME/clipboard integration, true removal of the original desktop window, and reconnect restoration remain incomplete. Actual EDA application menus, colours and mixed-DPI behavior still require wider validation. Remote desktop and SSH+X11 remain available alongside this feature.

Files with non-UTF-8 names cannot yet be opened or transferred through AetherDesk. Recursive operations involving them are refused rather than silently omitting files or changing their names.

The reported idle hang and intermittent server-stream disconnections have not been reproduced to a confirmed root cause. This release includes targeted fixes and improved diagnostics; complete resolution is not claimed. During native-capture recovery, Windows may handle some system shortcuts locally.

## Downloads and updates

Use the Windows x64 installer or extract the complete portable ZIP. Existing users can open the gear menu, choose **Updates**, and select **Check now**. Save work and disconnect sessions before installing an update. AetherDesk does not close your remote applications as part of updating the Windows client.

Windows and Linux compiler regressions and the bundled-X11 package test passed for this build. Clean-machine, real upgrade/restart and comprehensive live detached-window acceptance remain pending.
