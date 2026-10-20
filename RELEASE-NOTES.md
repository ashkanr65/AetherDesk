AetherDesk 0.1.2 build 5 — Portable updates and responsive windows

- Handle read-only bundled runtime files during portable updates and recovery.
- Flush each replacement to a temporary file in the destination folder before replacing the old file; incomplete copies preserve the original.
- Continue restoring other files when one recovery file is locked, and keep the failing paths and Windows errors in the recovery folder.
- Restart the previous portable application after a safely recovered replacement failure or failed archive extraction.
- Show horizontal tracks and filled progress on Application scale and Application font size sliders in Preferences.
- Fit oversized application windows when moving to a smaller or differently scaled monitor, while preserving the selected font and application scale.
- Reflow welcome text and connection cards at narrow widths. Keep header controls and the clock separate from truncated status text.
- Preserve the preferred sidebar width across temporary window/monitor changes and use a drawer when two panes cannot fit.
- Adapt Preferences navigation to smaller windows and provide bounded scrolling for dense connection and terminal settings.

Validation status:

- Build 4 passed its build checks, but the user's real portable update exposed a read-only-file failure affecting replacement and rollback. Those earlier checks did not prove successful upgrades.
- The completed build passed all seven stages in 39 minutes 6 seconds on 20 October 2026. Windows 282 registered / 256 required and Linux 95 / 80 regression checks passed, together with packaged X11 verification.
- All three new Windows filesystem regressions passed: read-only package files, interrupted writes and recovery with a locked file.
- All five new layout regressions passed: monitor/DPI changes, maximized/fullscreen windows, sidebar restoration, narrow welcome text and compact Preferences. The reported button/shortcut overlap is fixed and its original regression passes.
- The package matches the tested source fingerprint. Its completed executable passed isolated dark/light rendering and editor popup open/close checks without SDK tools on PATH.
- Real automatic two-version replacement/restart, settings and credential preservation, and clean-machine acceptance remain separate pending checks. These build results do not claim those tests passed.

Updates use the existing Ed25519 verification key and ask before restarting. Windows Authenticode signing remains separate and unconfigured.

Existing failed update folders must be retained until recovery has been verified. Older clients with read-only installed package files may need repair or a manual upgrade before they can receive this fix automatically.
