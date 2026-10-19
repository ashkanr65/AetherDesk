# R12 acceptance — 2026-10-14

This report distinguishes implemented features, automated checks and live acceptance.
Only this Windows PC and the existing Cadence SSH host are available. The existing
Windows Sandbox supplied disposable clean Windows environments for package checks;
no optional Windows features or developer SDKs were installed. Physical mixed-DPI
hardware and additional live-provider approvals are unavailable. All checks run
to completion passed; the broader manual/equivalence checks below remain open.
The replacement release and requested `0.0.1.AI-R` label remain pending.

| Area | Evidence/status |
|---|---|
| Windows compiler/regressions | Updated run: 211 passing tests; explicit live/package checks are ignored in ordinary builds. |
| Linux agent/regressions | Updated run: 91 registered native tests passed, including scoped GVFS mount matching. |
| Bundled X11 ownership/authentication | Package manager test passed; live xclock forwarded with authentication and owned cleanup. |
| SSH | Native key authentication to fic/AR; wrong fingerprint rejection, separate sessions and PTY resize passed. |
| Isolated terminal input | Five commands displayed without a subsequent keystroke; timings saved in live-ssh-sftp.json. |
| SFTP/editor/SSH AI tools | 1 MiB binary exact round trip; UTF-8 remote save; stale revision rejected; scoped command and independent connection cleanup passed. |
| Remote desktop | Scoped GVFS unmount fix passed the live retest: 1366×768, 2560×720/two virtual monitors and 1280×720 frames, retained reconnect, end acknowledgement and removal of the owned folder. First frame: 5666 ms on this LAN/host. |
| Live AI | Five explicitly approved OpenRouter requests total. Catalog/authentication, streamed tool call and native read_file round trip passed. Initial 32-token final reply check failed; 512-token retest passed. Only an isolated dummy file was sent. |
| Renderer/editor window | Final native dark/light render and real popup-close lifecycle passed on PC and clean Sandbox; main window kept rendering. Three broken/unsupported connection glyph labels were corrected and visually rechecked. |
| Portable ZIP | All 7156 extracted files matched the verified package by SHA-256; extracted app launched and passed rendering/popup checks without the SDK PATH. Clean Sandbox portable and bundled X11 tests passed. |
| Installer | Built successfully. Private installation on this PC matched all package files, launched, passed popup checks and uninstalled its registration. Fresh Sandbox installation, rendering/popup checks and complete uninstall also passed. |
| Full original-feature equivalence | Automated catalog coverage is not full live UI coverage. Manual checklist below remains open. |
| Clean-machine acceptance | Clean Windows Sandbox package launch passed, including portable X11 and installed UI. This proves dependency/startup checks in the tested VM, not every physical Windows machine or all network/hardware workflows. |
| Comparative performance | SSH timings/desktop startup measured; no matched original-app baseline, so no speedup claim. |

## Reproducing explicit checks

`tools/rust-build-kit/Run-Acceptance.ps1` accepts a non-secret JSON configuration
and a verified extracted package. It compiles an ignored harness and runs selected
checks; ordinary builds never connect to real hosts/providers. Default checks are
SSH, X11, Desktop and UI. Select Provider explicitly only when the user approves
the destination/model and dummy payload. Provider checks make three operations
(catalog and two chat requests), or two if providerCatalogPreviouslyVerified is true.
Credentials are referenced from Windows storage, never recorded in the configuration.

Configuration fields: host, username, identityFile, hostFingerprint, sessionId
(a new disposable UUID), reportDirectory, providerId, base and model. The runner
replaces reportDirectory with a timestamped `output/acceptance/r12-*` directory.
This evidence is kept when the build kit cleans generated packages/reports.

The UI self-check is invoked explicitly with `--r12-ui-check` and requires
APPDATA to equal `<AETHERDESK_R12_UI_ROOT>/profile`. It renders only its own
window and uses an isolated test file; it cannot run with the normal user profile.
No arbitrary desktop capture, host connection or provider request occurs there.

## Remaining acceptance jobs

| Job | Checks still required | Needed environment/input |
|---|---|---|
| R12.1 | Interactive terminal IME/AltGr/mouse/clipboard/scrollback/search and vim/top; password/encrypted-key authentication, fingerprint prompt UI and live WireGuard | Relevant keyboards, approved test credentials and configured VPN endpoint |
| R12.2 | Full file menus/associations, KLayout/Open With, Explorer drag/drop, interrupted transfer queue/retry/resume and external-file recovery | Local associated apps and disposable local/remote file fixtures |
| R12.3 | Dirty popup-close choices, advanced editor/search/diff and language-server completion/hover/workspace edits | Representative language servers and disposable projects |
| R12.4 | Physical/system-key forwarding, real multiple monitors/mixed DPI, docked toolbar dragging and host-window repositioning | Physical monitors/input hardware and supervised fullscreen testing |
| R12.5 | Real-profile/master-password migration, realistic histories, provider CRUD and NVIDIA/custom API vision/reasoning/error behavior | User-approved copies/unlock and explicitly approved additional provider requests |
| R12.6 | Representative EDA X11 windows/icons/clipboard, multiple hosts and interference against an independent external X server | Licensed apps/test hosts; preserve existing GUI/design locks |
| R12.7 | Full tabs/notifications/logging/fonts/sidebar, screenshot/approval UI, reusable Tools authoring/run/cancel and restart recovery | Supervised interactive workflow acceptance using disposable data |
| R12.8 | Matched original-versus-Rust startup/input/frame/CPU/memory measurements | Same machine/host/workload and a running original-app baseline |

## Final package evidence

Both artifacts use snapshot `2ff4322aaaa0bcba5f0ea96454edd34265bc01d45f34ba6390f9348bc918f0fb`.
Later edits update these status documents only; runtime source did not change.

- ZIP: `AetherDesk-Rust-preview-0.1.2-build1_20261014_154753_260_20261014_155033_405.zip`.
  SHA-256: `3C34266592AA448AD93F6F28CE225EE422FC5118005BDEDADCEFFF772776C25B`.
- Installer: `AetherDesk-Rust-preview-0.1.2-build1_20261014_154753_260-Setup.exe`.
  SHA-256: `38FA4108E0E86EBCC7F1124DCE44A57F9DC9A2F3A8BF83DCD27870A07711DC45`.
- `output/acceptance/initial-r12/r12`: live SSH/SFTP/X11/desktop/provider reports.
- `output/acceptance/r12-20261014-154940`: final packaged UI report/screenshots.
- `output/acceptance/packages-20261014-155327`: all-file ZIP/install comparison and PC install/uninstall evidence.
- `output/acceptance/sandbox-20261014-160302/results`: clean portable UI and X11 pass; its installer was interrupted by the initial short harness deadline.
- `output/acceptance/sandbox-20261014-160828/results`: targeted clean installer/UI/uninstall pass with a realistic timeout.

An earlier Sandbox harness did not retain GUI exit codes; the application UI report
passed and the runner was fixed. These were harness corrections, not suppressed
application failures. The source-owned runner uses the working process-wait path.

Do not mark these items passed based solely on source tests. Keep replacementReady
false until their acceptance evidence is supplied.
