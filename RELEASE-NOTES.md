# AetherDesk 0.1.2 build 8

This update improves native window sizing and remote display delivery, with particular attention to large and multi-monitor desktops.

## Window and workspace improvements

- Main, Files, Editor and review windows share monitor-aware sizing. Size corrections wait for dragging and display-scale changes to settle, while preserving normal window size and Windows snapping/maximizing.
- Narrow Files windows switch between local and server views, keeping navigation controls and file contents within their panes. Editor and review controls remain reachable in smaller windows.
- Automatic rendering on Windows now uses Direct3D 12 to avoid the Intel OpenGL driver path involved in a reported crash. An OpenGL compatibility option remains under Preferences > Display and requires a restart.
- All four home-page Ready indicators use the same information colour.

## Remote desktop improvements

- New sessions with the updated agent negotiate lossless changed-region delivery. Exact BGRA pixels preserve colours and fine graphical details; unchanged areas and tiles entirely in gaps between monitors are not transmitted.
- Bounded updates publish complete image transactions, with full visible refreshes after resize, reconnect or recovery. Older agents continue using compatible video delivery.
- Host capture follows delivery capacity. Compatible video delivery avoids an extra full-image copy and uses bounded parallel colour conversion.
- Fullscreen and all-monitor desktop windows remain open when switching to another connection tab. Returning to a tab selects the correct connection, and foreground checks protect keyboard routing.
- Bounded graphics, window-sizing and display timing diagnostics support troubleshooting without logging screen pixels, keystrokes, chat or terminal contents.

## Build reliability and verification

Workspace lock preparation now validates the complete dependency graph before locked builds. Corrected lossless test fixtures exercise valid desktop sizes without weakening production validation.

The completed build passed every build stage, Windows and Linux regressions, and bundled X11 runtime verification. The packaged application also passed an isolated rendering and Files/Editor resize, minimize, restore and close check.

## Updating and remaining verification

Install with the Windows x64 installer, extract the complete portable ZIP, or check for updates inside AetherDesk. Update metadata is signed with the established release key; the installer is not Windows Authenticode-signed.

Existing remote desktops retain their running agent. To use the new lossless path, save your remote work before ending the old remote session and creating a new one with the updated agent; reconnecting to an existing desktop alone does not replace its agent.

Live EDA colour/dialog checks, mixed-DPI monitor dragging, measured tab/fullscreen/all-monitor performance, real automatic replacement/restart with preserved user data, installer upgrade and clean-machine acceptance remain pending. The checks above do not establish a measured speedup or prove the original graphics-driver crash cannot recur.
