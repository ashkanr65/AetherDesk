# Rust conversion status

Updated 2026-10-14. Work is confined to `AetherDesk-Rust` and its separate build
kit. The production `AetherDesk` remains unchanged. The native executable is
still named `aetherdesk-rust-preview`; `replacementReady` remains false.

## Source implementations added

- Direct production SSH/SFTP/WireGuard backend, fingerprint confirmation,
  independent connections, terminal rendering/input and bounded cleanup.
- Bundled authenticated X11 startup, separate cookies, connection ownership,
  invalid-cookie probing and local child cleanup.
- Production remote agent/session/protocol/decoder pipeline connected to native
  tab/fullscreen/all-monitor rendering, frame capture, clipboard, keyboard/mouse,
  host resize, edge-docked toolbar and acknowledged End remote session.
- Stable primary saved-desktop identities and separate duplicate-tab identities.
- Separate native saved profiles, editing/bookmarks/trash/import/export; Windows
  Credential Manager plus DPAPI and optional PBKDF2-derived master protection.
- Dual editable-path file panes, file/folder transfer, pane drag/drop,
  progress/cancel, remote file menus and quoted path insertion into the terminal.
- Independent editor popups, encoding preservation, dirty-close protection,
  conflict-checked local/remote saves, local associations for binary files,
  explicit working-copy upload and SSH language-server diagnostics/completion/hover.
- Native provider HTTP/SSE and tool loop; searchable services/models of all price
  tiers, credential deletion, scoped approvals, saved/deletable chat history,
  screenshot attachments and approved application actions.
- Native reusable Tools manifests, revision review, quoted Windows/SSH parameters,
  result/cancel UI and Windows process-tree cleanup jobs.
- Native Application/Host/Audit/Problems logs, original asynchronous redaction and
  rotation, filtering/follow/export; persistent main theme/sidebar preferences.
- Native build recipe now includes agent tests and packages the Linux agent,
  GStreamer/MSVC and authenticated X11 runtimes for both installer and portable ZIP.

## Still incomplete

This is substantial source progress, not a claim that every feature is finished.
The detailed 56-group catalog in `RUST-FEATURE-PARITY.md` identifies the exact gaps.
Automated Windows/Linux baselines now pass, together with live SSH/SFTP saves,
X11, desktop geometry/reconnect/end cleanup and an OpenRouter file-tool exchange.
Installer/portable launch and editor popup closure also passed in clean Windows
Sandbox environments. Remaining acceptance includes physical input/mixed DPI,
full terminal/file/editor/Tools/log workflows, additional authentication and
WireGuard connections, representative EDA applications, real-data migration,
additional provider capabilities and comparison with the original application.
See `R12-ACCEPTANCE.md` for the numbered remaining checks and evidence.

Compilation stays with the user except for their explicitly authorized one-time
verification and subsequent R12 acceptance run on 2026-10-14. The first run compiled and tested the Windows application
and Linux agent, verified the bundled X11 runtime, and created the portable ZIP.
The installer was not built in the first run. R12 subsequently built both packages
and tested installation, launch and uninstall. Earlier migration passes did not compile.
Formatting and offline recipe checks are not substitutes for compilation or live
Windows/SSH/X11/desktop/provider regression testing. No speed improvement is claimed
without measurement.

## Numbered job list

Update this table after each requested job. "Implemented" means source work is
complete; compiler and live acceptance checks are tracked separately in R12.

| Job | Work | Status |
|---|---|---|
| R1 | Connection cancellation, SSH command process cleanup and duplicate retained-desktop recovery | Implemented; automated checks and key-auth/session isolation/retained-desktop live checks passed; full lifecycle UI acceptance remains R12.1/R12.7 |
| R2 | Terminal mouse/IME/AltGr, settings/presets and current-folder synchronization | Implemented; automated checks and isolated shell input/PTY resize passed; interactive acceptance remains R12.1 |
| R3 | Local file menus, Open With, comparison and external-file recovery | Implemented; automated checks passed; full OS/menu/association acceptance remains R12.2 |
| R4 | Transfer queue, retry/resume and Windows Explorer drag/drop | Implemented; automated checks and live binary round trip passed; interrupted queue/Explorer acceptance remains R12.2 |
| R5 | Editor syntax/languages, indentation, advanced search/diff and remaining language edits | Implemented; automated checks, popup lifecycle and live UTF-8 save/conflict passed; language/workflow acceptance remains R12.3 |
| R6 | Remote desktop Windows keys, mixed DPI and saved toolbar placement | Implemented; automated geometry/input checks and live decoded monitor layouts passed; physical hardware/UI acceptance remains R12.4 |
| R7 | Original profiles/recovery, credentials, settings and AI history migration | Implemented; native fixtures passed; real-data migration acceptance remains R12.5 |
| R8 | AI provider capabilities, reasoning/output limits, custom OpenAI-compatible APIs and per-chat/large-history state | Implemented; transport/model/history/approval fixtures and live OpenRouter streaming/file tool passed; broader provider/UI acceptance remains R12.5/R12.7 |
| R9 | X11 shared runtime, icons and Windows integration parity | Implemented; bundled ownership/authentication tests passed on PC and clean Sandbox; live xclock passed; EDA/clipboard/external-server acceptance remains R12.6 |
| R10 | Tabs/navigation, notifications/counters, fonts/icons and log/UI parity | Implemented; automated checks and dark/light/popup rendering passed; full workflow acceptance remains R12.7 |
| R11 | Complete original-behaviour regression coverage and build-kit checks | Implemented; 56 groups/37 suites mapped; 211 Windows and 91 Linux native tests passed; original equivalence still requires the remaining R12 checks |
| R12 | Compiler/regression/live tests, clean-machine packaging and performance measurements | Partial acceptance: available automated/live/package/Sandbox checks passed; R12.1-R12.8 remain manual/hardware/provider/comparative acceptance, detailed in R12-ACCEPTANCE.md |
| T1 | Solid bronze/black and cream/gold themes with native layout; background images removed | Dark/light rendering and visible sidebar/connection glyphs verified in final portable and installed builds, on this PC and clean Windows Sandbox |

## Release naming requested after acceptance

The requested first replacement output label is `0.0.1.AI-R`: numeric version
`0.0.1`, with `AI-R` identifying AI support and the Rust implementation. Activate
this label only after R12 passes; the current preview keeps its existing name.
Keep Cargo and installer numeric versions separate from the output label. Increase
the numeric release for major updates; routine rebuilds use the build counter.
The label change remains pending while acceptance has unresolved items.

## R10 implementation and acceptance

- Added the original Ctrl+Tab/Ctrl+Shift+Tab, Ctrl+PageDown/Ctrl+PageUp,
  Ctrl+Shift+K and Ctrl+Shift+B workspace shortcuts. Readline/editor and AltGr
  combinations are preserved. Physical desktop capture continues to send remote
  system shortcuts to the host; restore to tab or focus the toolbar for local
  workspace navigation.
- Independent tab identities now drive a horizontal, scrolling tab strip with
  active-tab reveal, drag reorder, previous/next buttons, middle-click close and
  per-tab state. Reordering preserves the selected session. Close all tabs detaches
  retained desktops and releases each tab's own connection.
- Active Connections shows live/opening sessions only, selects the existing tab,
  and disconnects individual sessions. The status bar includes active/connected
  counts, connection/authentication types and a ticking clock.
- Added a bounded notification queue with five-second lifetimes, a 180ms fade,
  dismiss control and application-log mirroring. Connection/file success and
  failures from background tabs reach the main window without changing selection.
- Retained the installed Windows UI/heading fonts, terminal font preferences,
  vector controls, product icon and visible selection colours in both solid
  themes. No background images were restored.
- Log views now parse severity/source/target, include rotated files, merge
  timestamped records, select host desktops independently of the active tab,
  filter levels/source/case, show counts, copy visible records, open the log
  folder and export displayed records to a new file. Disk reads, SSH requests,
  folder opening and export run off the UI thread. Tail/record limits and virtual
  rows bound memory and rendering; exports redact sensitive fields.
- Added regression cases for navigation identity/wrapping, shortcut isolation,
  notification overflow/lifetimes, rotation/filter/redaction and bounded tails.
  These Rust tests are supplied for the build kit; compilation stays with the user.

R12 acceptance for R10: test hundreds of tabs, duplicate names, drag/reorder,
closing selected/nonselected tabs, keyboard switching from terminal/file/chat
focus, sidebar width/reset after restart, both themes at mixed DPI, background
connection notices, clock/counters, all log views, disconnected host selection,
case/source/severity filters and copy/export during rotation.

## R11 regression coverage and build-kit checks

One-time portable build acceptance, 2026-10-14: 211 Windows regressions passed,
the Linux agent suite passed (90 compiler-registered tests), and the explicit
bundled X11 runtime regression passed. The release executable and verified
portable ZIP were created. ZIP contents and its recorded SHA-256 were checked.
Packaging's duplicate X11 source download stalled; an existing source/licence
cache was verified against the unchanged pinned hashes and reused, then only
stages 4 and 5 were resumed with the successful compiler records preserved.
The incomplete package and cancelled download's empty partial file were removed.
Output evidence: `AetherDesk-Rust-output/reports/portable-verification-summary.json`.
Live hosts, real providers, clean-machine operation, installer and performance
acceptance remain R12. No further compilation is authorized by this exception.

Authorized compiler verification, 2026-10-14: the real AlmaLinux moving-colour
test was reproduced with native compilation. Older `h264parse` reports the old
duration on the first packet after a frame-rate change even when the packet's
SPS/caps advertise the new rate. Encoded packet duration now follows its own
negotiated caps, preserving queued packets' individual rates. The unchanged
90-frame colour/30-to-5-to-30-fps regression passed after the fix, including
keyframes, timestamps and image-quality assertions. This compiler/build run is
authorized for this request only; live feature acceptance remains R12.

Legacy libav compatibility correction, 2026-10-14: the AlmaLinux 8 H.264 decoder
lacks `thread-type`, so the unconditional slice-thread setting panicked in four
real-video regressions. Decoder setup now checks that the property exists and
accepts `slice`; otherwise it uses the older supported `max-threads=1` setting
to avoid waiting for a following frame. Missing threading controls return a
pipeline error instead of panicking. Newer Windows/video runtimes retain slice
threading. The existing isolated-update regression additionally checks the
single-worker setting on older runtimes; colour/resize regressions remain enabled.
Rust formatting, offline build-kit fixtures, migration inventory and whitespace
checks passed. No compilation was run; compiler/runtime verification remains
R12 and stays with the user.

Xwayland source-download correction, 2026-10-14: curl error 35 was an upstream
TLS/connection failure during the pinned source download. The AlmaLinux 8 curl
retry option did not cover it. The downloader now explicitly retries all failed
transfers with bounded connection/transfer/stall timeouts and tries an independent
OSUOSL BLFS HTTPS mirror before repeating the failed host. All five mirrored
archives were downloaded separately and matched the unchanged pinned SHA-256
hashes. HTTPS certificate checks remain enabled; corrupt/partial files are
discarded and only verified archives are extracted. Eight offline fixtures cover
SSL retries, mirror fallback, corrupt/valid cached sources, checksum failures,
exhaustion and invalid requests. The container runs these fixtures before its
Xwayland build. Compiler/container/package acceptance remains R12; no build ran.

Build output cleanup, 2026-10-14: full, portable-only and rebuild launchers now
clear previous generated packages, installer files, logs, reports, snapshots,
stage records and latest-package pointers once under the build mutex before
starting workers. Reusable dependency/compiler caches and unrecognized user
files remain. Individual `-Step` runs retain prerequisites. Cleanup rejects
source/ancestor/drive-root targets and artifact junctions before deletion.
Offline fixtures exercise deletion and preservation using private temporary
folders only. Compiler/live/package acceptance is still R12; no build was run.

Linux video dependency correction, 2026-10-14: the AlmaLinux build container now
explicitly installs RPM Fusion's `gstreamer1-libav`, which supplies `avdec_h264`
for the real decoder/color/isolated-update regressions. It checks appsrc, appsink,
videoconvert, h264parse, x264enc and avdec_h264 immediately after package setup,
before costly compilation. The decoder regressions remain required. This fixes
the shared test stage used by both portable and installer builds. Offline recipe
and migration checks passed; container/runtime verification stays with the user.

Regression correction, 2026-10-14: screenshot encoding now preserves dimensions
and exact colours for captures already within 1600 pixels, instead of resampling
and enlarging them. The fixture also checks the encoded PNG and a tall capture.
The Windows job fixture now checks live parent/descendant processes before job
close, bounded termination afterward, and survival of an unrelated gated process.
It no longer assumes a particular termination exit code; its process handles are
released on assertion failure. Both tests remain required. Formatting/migration
checks passed; the user will rerun compilation and the corrected tests in R12.

Platform correction, 2026-10-14: three decoder regressions were incorrectly
assigned to the Windows catalog despite their Linux/non-Windows compile guards.
They now run in the Linux container, which includes `ad-display-decoder` in both
registration and execution. Windows still requires its software-H264 policy test.
Preflight rejects explicit platform/phase mismatches; offline fixtures cover both
directions and verify that the Linux recipe registers and runs decoder tests.
Compiler/container verification remains with the user in R12.

Build correction, 2026-10-14: the Saved desktop tabs window now keeps its
open/close flag in a local variable and clones the matched saved profile before
the row callback. Error notifications can mutably access the application without
overlapping either borrow, fixing the reported E0500/E0502 errors. Formatting
and migration checks passed; compiler verification stays with the user in R12.

- `rust-regressions.json` maps all 56 behavior groups, all 30 original Flutter
  test suites, three original Windows native suites and four original helper/
  recipe suites to native behavioral checks and explicit live acceptance. The
  catalog is a coverage map, not evidence that tests passed.
- Added real loopback HTTP fixtures for custom paths/keys, paid/free model
  catalogs, streamed Unicode, transient retries, interrupted output, redirects
  and missing models. Local model requests bypass network proxies. Added model
  capability/context fixtures that keep tool turns intact.
- Added isolated disk fixtures for atomic stores, corrupt-data preservation,
  profile import/export and backup recovery, lazy chat metadata/messages,
  deletion of stored images and safe transcript migration. Added screenshot
  colour/stability/limits, editor close/dirty/busy ownership, application-action
  identity, Credential Manager isolation/deletion, and Windows process-tree
  cleanup cases. Test data stays in temporary directories or dedicated random
  native credential targets; original user data is not used.
- The regressions exposed and fixed duplicate history IDs being written,
  inconsistent settings filename validation, malformed capture frames,
  missing-primary profile backup recovery and first-notice loss during bursts.
- Stage 2 now lists compiled Windows tests, rejects missing required
  registrations, runs UI/core suites through their own locked workspaces, and
  requires nonempty passing results and sufficient baseline counts. It records
  a catalog hash and distinguishes compiler success from live acceptance.
- The Linux container checks compiled registrations and runs Wayland,
  supervisor, input, encoder and SSH suites, real local SFTP fixtures, scoped
  job cleanup fixtures, icon-helper tests and registry-validator fixtures. Its
  report is bundled with the agent.
- Stage 4 requires one passing bundled X11 test; a misspelled/zero-test filter
  cannot count as success. Packages contain `verification/` evidence and a
  matching catalog hash. ZIP/installer stages reject missing/mismatched Windows,
  Linux or bundled-X11 results. Existing source/artifact fingerprints invalidate
  stale outputs on retry.
- Offline recipe fixtures passed with Cargo mocked, including missing/empty
  registries, zero-test success, failures, report state and artifact integrity.
  The registry validator's two Python fixtures passed. The icon helper's local
  fixtures passed with its environment-dependent icon test skipped. Frozen
  references, key-map checks and formatting passed.

All Rust/compiler, native Windows, container, provider/live-host, installer/ZIP
and performance acceptance remains R12 and was not run here. No compilation or
application build was performed. `replacementReady` remains false. The source
job list R1-R11 is implemented; R12 must pass before claiming full replacement.

Build correction, 2026-10-13: R4's `resumable_file.rs` is a sibling of
`client.rs`, so its nested module declaration now uses an explicit `#[path]`,
matching the other SSH helper modules. The offline recipe validates module
resolution and rejects the previous broken declaration. Compiler/live
verification remains pending; no compilation was run for this correction.

Desktop compiler corrections, 2026-10-13: the egui 0.33.3 IME output type is
referenced through `egui::output`; comparison colours are captured before the
mutable columns layout; language-service availability is recorded before moving
its authenticated session into the launch task. Stroke widths use explicit
`f32` literals to remove the reported future-compatibility warnings. R2/R3/T1
compiler and runtime verification remains pending with the user.

## R1 implementation and verification

- Startup cancellation keeps launch ownership until its bounded reply, then
  closes partial transports. A fresh, positively identified agent is terminated
  with acknowledgement; an existing desktop is detached and preserved.
- Fresh agents that never reach Start/Resume expire after a 60-second claim
  grace period. Successfully attached desktops retain their unlimited detached
  lifetime. Cancelled X11 startup waits for local server cleanup.
- Reconnect is gated until the previous connection worker finishes cleanup;
  application shutdown allows that bounded cleanup to finish.
- AI/Tools SSH commands use an isolated process group, an input gate and a
  renewable stdin lease. Completion, cancellation, timeout, excessive output,
  EOF or lost heartbeats release the group. TERM is followed by KILL; unrelated
  host processes are never selected by name. Linux Bash and util-linux setsid
  are required for this command path. Explicitly daemonized services that leave
  the group are not a process-group sandbox and require their own service stop.
- Duplicate desktop IDs are saved atomically before launch, including the
  connection/account identity, in native `desktop-recovery.json`. Username
  storage respects the saved connection's consent; an account hash binds a
  recovery bookmark when the username must be re-entered. "Saved desktop
  tabs" reopens those IDs after tab close/restart, focuses an already-open tab,
  and removes its record only after confirmed End. Records are recovery
  bookmarks; they do not assert that a host desktop still exists.
- Eight interpreted Linux job fixtures passed locally: input gate, EOF,
  TERM-resistant children, normal completion/status/output, fragmented lease
  messages, long-job renewal, silent lease expiry and supervisor termination.
  Formatting, manifest metadata and offline recipe checks passed. Added Rust
  regressions cover startup phases, existing-desktop preservation, recovery
  identities, reconnect cleanup state and unclaimed-agent expiry. These Rust
  tests were not compiled or run.
- Build-kit stage 2 runs the new Rust tests through the existing workspaces;
  its Linux container also runs agent-supervisor tests and the interpreted job
  fixtures. No stage/application/package build was run during R1 implementation.

R12 live acceptance for R1: cancel during deployment/launch/handshake, repeatedly
close/reconnect SSH/SFTP/X11 tabs, stop AI/Tools scripts with children, restart the
application and reopen duplicate desktops, then confirm End removes only the
chosen desktop and its recovery record. Confirm host process counts return to
baseline for ended connections while deliberately detached desktops stay alive.

## R2 implementation and acceptance

Terminal input now handles default/UTF-8/SGR mouse reporting, button/motion/wheel
modes, mouse release on tab/focus loss, Shift selection/scrollback, IME preedit
and commit deduplication and AltGr printable text. The four original terminal
presets and eleven semantic roles are preserved in JSON. Settings save font,
font size, line height, scrollback capacity, semantic colours and folder following.
Changing scrollback capacity preserves the visible screen and resets its history;
the settings dialog explains that reset. Explicit ANSI styling wins over automatic
plain-line classification.

Bash prompt integration reports OSC7 without replacing existing prompt commands.
Split reports, URI decoding and rejected control characters have source regressions.
The file pane follows shell reports; explicit file-pane navigation can change the
shell directory through a validated, quoted user action. Escape reports themselves
never execute a command. Other shells retain normal operation and may provide
their own OSC7 integration.

R12 acceptance: exercise vim/top mouse modes, Shift selection, wide cells, AltGr,
IME composition, Readline/clipboard keys, all presets, restart saved settings,
folders containing spaces/apostrophes/percent signs and split prompt reports.

## R3 implementation and acceptance

Both file panes have Open, text editor, local Windows default application,
Windows Open With, comparison, rename/delete/new folder, path copying and
terminal insertion. Local properties/security use Windows dialogs; server
properties and Unix permissions use SFTP. Open documents protect their containing
folders across sessions of the same authenticated account. New destinations and
rename collisions are handled separately; root deletion and link/junction traversal
are rejected. Comparisons show read-only paired lines and full byte/hash equality;
aligned editor comparisons were added under R5.

Remote working copies and native editor drafts survive restart, bound to the
original host, account, key, canonical path and server revision. Use the menu's
Recover working files with that host connected. External edits are detected;
native text is preserved until an explicit reload, and stale saves are refused.
Uploading an external editor's saved copy uses an immutable snapshot and updates
the recovery baseline without replacing any newer external edits. OS application
launch and picker work run off the UI thread. Editor windows remain independent.

R12 acceptance: repeated popup open/close; cancel the application chooser; edit
GDS files in local KLayout and upload explicitly; recover after restart; change
the server file during editing; rename/delete an open document's ancestor; compare
local/local, local/server and server/server files and reject link/root operations.

## R4 implementation and acceptance

The durable queue runs up to four jobs and exposes Pause, Resume/retry, Cancel
and Discard partial files. Pane drag/drop and Windows Explorer drops use it,
including bounded recursive folders. Existing files are preserved by default;
replacement requires an explicit revision-checked choice. Queue state and source/
destination hashes are saved before transferring file data. Resume verifies the
complete source and existing partial prefix, publishes only complete verified
files and recognizes a previously published file after a lost reply. Concurrent
jobs targeting one file share a publish lock. No retry refreshes a conflicting
destination revision or silently changes hosts.

Restart restores unfinished jobs paused. Resume requires the same authenticated
host/account/key. Cancel preserves stages until explicit cleanup, which removes
only that job's private stage files, never published destination files or created
user folders. Corrupt queue/plan files are preserved and reported. Folder limits
are 128 levels / 100,000 entries; incompatible Windows names, name collisions,
symbolic links and junctions are refused. Retry is user initiated.

R12 acceptance: pause mid-file; close/reconnect a tab; restart the app; resume
both directions; transfer nested/empty folders; change source or destination;
lose the publish reply; cancel/discard; drop Explorer files inside/outside the
server pane; run four transfers while typing and playing remote video. New source
regressions exercise resumable prefixes and conflict refusal using a local
OpenSSH SFTP server in the build container. They were not compiled/run here.

## R5 implementation and acceptance

Native editor popups now have an open-document strip, extension detection and
manual language selection, embedded multiline syntax colouring (including
Verilog/SystemVerilog, Tcl, PowerShell and TypeScript), adjustable font, line
numbers, wrap, tabs/spaces, indent/outdent and automatic newline indentation.
Ctrl+F/H opens search, F3/Shift+F3 navigates matches; case, whole word, regex,
capture replacement and search in a selected range are supported. Ctrl+Z/Y and
Ctrl+Shift+Z use bounded undo history covering typing, replacements, indentation,
completion and language edits. Go-to-line and visible match selection scroll
inside the independent popup. Encoding/BOM and existing line endings remain in
the original TextFile/save pipeline. Ctrl+S retains conflict checks.

File comparisons and Compare with saved use aligned inserted/deleted/replaced
rows with original line numbers, synchronized scrolling and change navigation
in a native comparison popup. Diff and syntax work run off the UI thread. Diff
is bounded to 200,000 lines per side with a two-second algorithm deadline (which
may approximate alignment on difficult inputs); full file byte hashes remain
visible in file comparisons. The 8 MiB document limit remains.

Local LSP processes and SSH LSP channels are explicitly configured/launched.
Executable/JSON-argument configuration is saved per language and verified host
identity; saved settings never automatically run a server. Added definitions,
formatting, symbol rename and completion additionalTextEdits. Ctrl+Space requests
completion, F12 opens a definition, and Alt+Shift+F requests formatting.
Language text edits
use checked UTF-16 ranges, versions, overlap and encoding/size validation.
Multi-file edits open target buffers on the original computer and show a review
with optional comparisons. A stale/closed/external or unsaved other target is
refused. Apply updates all reviewed buffers; saving is explicit and retains the
original file revision checks. LSP snippets and resource operations are not
advertised, matching the reference client's text-edit scope. Local framing and
queues are bounded; LSP messages retain the production 2 MiB frame limit (larger
documents remain editable, with an explicit language-service limit error).
Windows process jobs terminate owned server children on stop.

Added source regressions for Unicode/captures, CRLF indentation, invalid ranges,
additional completion edits, bounded undo/redo, fragmented/invalid LSP frames,
file-URI restrictions, aligned diff and syntax text preservation. Build-kit
stage 2 discovers these through the existing native crate tests. No compiler or
build was run for R5. R12 acceptance: test all shortcuts and selected search,
undo/redo after every edit type, empty/large files, LF/CRLF/UTF-16 BOM round trips,
local and SSH language-server completion/diagnostics/definition/formatting/rename,
review cancellation and changes made during requests/review, process cleanup,
repeated popup open/close and both solid themes at mixed DPI.

## T1 solid themes and verification

Updated at the user's request: reference background PNGs, texture loading,
cloud/circuit/compass artwork and image-based card frames have been removed.
Dark bronze/black and light cream/gold colours, native controls, selection contrast
and the home layout remain. The product window icon is retained. Offline packaging
checks reject theme background-image loading. Running-app/DPI verification remains
with the user under R12.

## R6 implementation and acceptance

Compiler correction, 2026-10-13: the Windows monitor collection now explicitly
declares `Vec<Monitor>`. Its raw-pointer callback cast did not determine the
collection element type for the sorting closure, causing E0282 in stage 2.
Formatting is checked; compilation and live verification remain with the user.

Physical Windows scan codes now preserve keypad Enter/navigation, left/right
modifiers, AltGr, Windows/menu keys, Print Screen, Pause and repeat edges.
Keyboard layout selection follows the original supported Windows layout map;
unknown layout IDs are not silently replaced with US. Capture runs on a separate
message-loop thread and queues bounded events without network calls in hooks.
Physical key and mouse events share one route to preserve modifier-click order.
Monitor/layout changes enter the same client writer queue before later inputs.

Capture requires the focused remote surface and its exact foreground window.
Other applications and the remote toolbar are excluded. Focus loss, tab/window
changes, a stalled UI lease and close release tracked keys and buttons.
Ctrl+Alt+Esc returns local keyboard control; in fullscreen it returns to the tab.
The explicit Ctrl+Alt+Del button sends that sequence to the Linux desktop.
Windows secure-attention input itself cannot be intercepted. Injected keyboard
and mouse events remain excluded; automatic approval review rejected broadening
keyboard capture because of capture/loop risk. The existing IME commit path is
retained, with native capture paused while IME is open; this does not introduce
new host Unicode/IME support beyond the production agent's capabilities.

Tab/current/all-monitor transitions send a complete validated host monitor layout,
including a single region on return to the tab. Native monitor origins and window
placement use physical pixels, including negative origins and mixed DPI. Display
snapshots refresh after changes; an unplugged selected device falls back to the
primary monitor. Odd dimensions are normalized to even encoder dimensions;
large arrangements are uniformly scaled to the existing protocol allocation
limits. The production decoder, clipboard and acknowledged session cleanup stay
in use.

The hamburger handle supports floating movement and all four edge docks.
The toolbar scales on its own monitor, including when moved across displays in
all-monitor mode. Ratios and the monitor device are saved in the separate native
remote-toolbar.json, so reconnect/resizing/removal do not restore stale physical
coordinates. Invalid saved data is preserved with an error and session-only
movement; it is not silently overwritten. This setting is shared across native
remote sessions. Dragging releases remote input and toolbar hit testing excludes
both current and anticipated toolbar geometry.

Added source regressions cover keypad/sided keys, AltGr, repeat/release, physical
pointer mapping, ordered configuration barriers, mixed-DPI negative origins,
large/odd layouts, missing devices, four-edge/floating placement, DPI-scaled snap
margins and invalid saved ratios. Build-kit stage 2 runs these with the existing
ad-desktop test suite. Formatting and offline recipe checks are separate from
these compiler-based tests, which were not run.

R12 acceptance for R6: use US/international and a non-US Windows layout; test
keypad versus ordinary Enter/navigation, right modifiers/AltGr, Caps/Num lock,
Win/menu/Print/Pause, modifier-click/drag/wheel and Ctrl+Alt+Esc. Switch focus to
another application while holding keys/buttons and confirm no stuck input or
input capture outside the remote surface. Exercise tab/current/all transitions
with different DPI, vertical/negative-origin arrangements, a display unplugged
while connected and a large layout that exceeds protocol limits. Check host
window repositioning and pointer alignment. Drag/dock on every edge of each
monitor, reconnect/restart, resize/change DPI, remove the saved monitor and test
invalid settings preservation. Recheck colour/video, clipboard, screenshot,
Detach/reconnect and acknowledged End to guard against regressions.

## R7 implementation and acceptance

The menu's explicit “Import original AetherDesk data” flow reads the Flutter
profile store, terminal-theme settings, assistant provider settings and
LangGraph transcript history without modifying or renaming any original files.
Profiles retain IDs unless an ID conflicts with an existing native profile;
conflicts receive new IDs. Terminal settings are mapped only when no native
terminal settings file exists. Provider endpoints and manually selected models
are copied, and history becomes plain user/assistant transcript text. Imported
images are marked as archived attachments and tool-action records are omitted,
so old commands and grants are never replayed.

Credential migration is separately opt-in. It reads the original Windows
Credential Manager namespace, supports the original DPAPI and
PBKDF2-SHA256/210,000 envelope, and stores successful imports under the Rust
application's separate credential namespace. The master password is held only
for the import operation. Protected/unavailable/conflicting secrets are counted
as skipped; API keys and SSH passwords never enter settings or history JSON.
Corrupt or oversized source files are reported without being changed.

R12 acceptance for R7: import with credentials unchecked, then checked; verify
profile fields, provider IDs/endpoints/models, each connection credential and
API key, terminal preset/colors and old conversation order. Exercise a wrong or
missing master password, a duplicate profile ID, malformed/oversized source,
existing native destination values and repeat import. Confirm original source
files and credentials remain intact and that transcript import does not restore
tool approvals or run tools. Windows credential and DPAPI verification remains
pending; compilation was not run.

## R8 implementation and acceptance

Provider model lists retain every returned model and now keep declared context
length, output cap, supported request parameters, tool/image support and
reasoning metadata. Models stay alphabetically searchable, including paid and
free models. The chat UI shows known/unknown capabilities, per-chat context and
output limits, tool toggle, supported reasoning efforts and an optional reasoning
budget. OpenRouter requests use declared token parameters and reasoning fields;
NVIDIA NIM can opt into its documented `chat_template_kwargs.enable_thinking`
switch. Output and context limits are constrained to the model's declared caps;
vision/tool-declared unsupported models are stopped before a request/action.
Undeclared capabilities remain unknown so models are not silently hidden.

Each chat now saves its provider/model, draft, local workspace, context/output
settings, reasoning and tool preference. Local workspace identity is restored
only when the folder still exists; remote chat scope is retained as an identity
and requires the matching live connection. History is indexed separately from
each chat's bounded message file; a selected chat loads on demand, the UI starts
with the latest 100 messages and can reveal older messages, and deleting/clearing
history removes native per-chat files. Original chat transcripts are imported
without replaying action calls.

Custom OpenAI-compatible providers accept a user-supplied HTTPS base URL, an
optional API key, and an optional exact model ID. The URL may be the API root,
`/v1` base, or a full `/chat/completions` endpoint. The exact model remains
selectable when the service omits it from `/models`; model discovery is optional,
so an API that only serves chat completions can still be used. API keys remain
in the OS credential vault, and an empty key field does not erase an existing
saved key when updating the same provider.

R12 acceptance for R8: check OpenRouter model capability catalogs and NVIDIA NIM
reasoning toggles against the chosen model, unsupported tool/image behavior,
output/context clamps, context trimming across tool rounds, and per-chat model,
draft, local/SSH scope, reasoning and tool restoration across switches/restart.
Load/delete/clear and reopen long histories, check corrupt/oversized chat files,
and verify provider failures do not replay tools. Add custom API checks with an
API-root URL, `/v1` URL and full `/chat/completions` URL; exercise a service with
no `/models` endpoint, a manually entered model ID, optional auth and key
preservation when updating the provider. No compilation or tests were run;
Windows/provider verification remains with the user.

## Review of R1-R8, 2026-10-14

Reviewed the native connection ownership/cleanup, terminal integration, file
revision/recovery and transfer paths, editor/LSP edit handling, remote input and
monitor mapping, original-data import, and AI provider/history/workspace paths.
This was a source review with formatting and offline recipe checks, not a
compiler or live-behaviour certification. R12 remains required for every area.

Concrete AI corrections made during this review:

- A named custom provider retains its model-ID field. Changing presets clears
  the previous draft key/model, and an explicit model can be used without an
  automatic catalog request. Manual models persist across refresh and restart;
  model sorting remains case-insensitive.
- The first restored chat now loads its saved provider, model, draft and
  workspace. A missing provider cannot silently become a different provider.
  An unavailable saved workspace remains recorded until an explicit selection;
  sending is blocked while workspace restoration or history loading is unresolved.
- Source regressions cover manual-model persistence/sorting and unavailable
  workspace restoration. These compiler-based tests were added, not run.
- Failed history deletions keep the affected chats visible and report the
  failure; clearing history resets restored workspace warnings for the new
  chat. Connected-service selection is disabled while a response is running.

Checks passed: native and relevant backend Rust formatting, frozen reference
hashes and 106 physical key mappings, the 56-group/475-file inventory, and offline
build-kit fixtures (Cargo is mocked in those fixtures). Snapshot coverage,
workspace routing, failure propagation, package notices and X11 test routing
were checked. Windows CRLF-aware whitespace checks passed. No application,
Linux agent, Rust test binary, installer or ZIP was compiled or run.

## R9 implementation and acceptance

All X11 tabs in one Rust application now lease one authenticated bundled VcXsrv
process. Concurrent startup is serialized; cancelling a waiting tab does not
stop another tab. Each SSH tab gets its own forwarding cookie, rewritten by the
existing production SSH proxy to the shared local cookie. Closing a tab releases
its lease; the last release waits for the owned process to stop. Startup/shutdown
are serialized against reconnect, and an old failed display cannot stop its
replacement. Windows kill-on-close jobs also contain the owned server's child
processes. Existing external X servers are never adopted or terminated.

Startup retains private authority generation, rejection of a bad cookie,
software GLX, clipboard, hidden tray and multiwindow mode. Process health is
checked without repeating authentication probes during normal use, so an X
server grab does not cause false disconnects. The private HOME now contains
`.XWinrc` and an embedded native ICO fallback. Existing application icons and
the production optional host-side icon helper remain in use. No theme background
image was restored. Forwarding targets IPv4 loopback; VcXsrv's authenticated TCP
listener may also bind other interfaces, matching the reference launch options.
Shared X11 applications remain mutually trusted clients of that display.

The source-owned build kit snapshots the native ICO. Stage 2 includes cancellation
and icon-settings source regressions. Stage 4 runs the real native manager
against the freshly bundled server before recording a successful package: two
concurrent leases, distinct forwarding cookies, bad-cookie rejection, core fonts,
private icon settings, single/last-tab release, restart after process exit and
old-lease cleanup. Its routing, environment restoration and failure propagation
passed offline fixtures; the real server test has not been compiled or run here.

R12 acceptance for R9: build and run the new stage-4 check, then connect multiple
SSH+X11 tabs to one and different hosts; open xclock and representative EDA
applications, verify supplied/fallback icons, clipboard and ordinary Windows
window controls. Close/cancel/reconnect tabs in different orders; only the final
X11 tab should stop the shared server. Verify optional icon-helper failure still
allows applications, server exit reports errors to affected tabs, a fresh
connection restarts it, and another independently running X server stays intact.

## Session UI corrections — 2026-10-15

The Rust client now has masked password/passphrase entry and encrypted credential
saving in its New/Edit connection dialog. Secrets remain transient or encrypted
in the Rust app's separate Windows Credential Manager namespace; profile JSON
and exports never contain them. Existing saved secrets can be retained when
editing unrelated fields; changing their server/account/authentication context
requires entering a replacement. Master-password processing runs off the UI thread.

Tabs retain their 190 × 30 size with an internal close target and no redundant
arrow controls. Scrolling, drag reorder, middle-click close and Ctrl+Tab remain.
Connection credentials and SSH host verification use floating dialogs instead
of inline forms. Dialog action buttons are adjacent.

Terminal semantic colours apply to matching words, with a separate whole-line
colour for lines beginning with #. Users can add/remove literal custom words or
phrases and choose their colours. The renderer maps UTF-8 ranges through actual
terminal cells, preserving explicit host ANSI styling and alternate-screen apps.
Settings have individually bordered controls, independent fixed-height columns,
and Apply/Cancel actions; cancelled edits do not change the running terminal.

The main toolbar has one dark/light switch. Its settings gear provides Settings,
Logfiles, Help and About/version. Logs are no longer a left-rail tab; approved AI
requests to show logs open the dialog without replacing the selected connection.

Progress is tracked in SESSION-UI-TASKS.md. Four new behavioural regressions are
registered in the existing Rust build-kit step 2. Source registry validation and
Rust formatting checks passed. No compiler, native test, application build or
package was run for these changes; the next user build must validate them.
### Session UI follow-up

The follow-up fixes are tracked in SESSION-UI-TASKS.md. Save and open completes
the credential write and persists session metadata before opening a tab. Saved
credentials load automatically for their bound username/server; master-protected
credentials require the master password. Password-save failures are surfaced,
and a completed background save does not overwrite a disconnected-session status.

The terminal has one compact action row, padded tabs, two bordered settings
columns, visible slider tracks and right-aligned Apply/Cancel controls. Custom
words live under Colours, and the Sessions/Bookmarks Trash checkbox is removed.
Bash folder synchronization uses a reserved command with a narrowly scoped
history exclusion. Startup clears the first prompt, and large initialization
commands are not embedded a second time in their history matcher.

Packages now use AetherDesk Rust and aetherdesk-rust.exe, with no Preview suffix.
The build kit, verifier, latest-output pointers, acceptance launcher and installer
shortcuts use the same names; upgrade cleanup removes the old executable and
shortcut. Historical acceptance reports retain the names they actually tested.
Release/feature acceptance flags remain unchanged. Offline recipe fixtures and
isolated real-Bash PTY checks passed; compilation/native UI tests remain for the
user's next build.

### Desktop, Files and latency follow-up

The session toolbar places desktop monitor actions beside Connect/Disconnect,
End remote session and Files in one horizontally scrolling row. The typing hint
is removed. Files opens a separate native viewport for SSH, SSH+X11 and desktop
sessions, leaving the main viewport geometry unchanged. Closing Files targets
only its own viewport and clears its file dialogs without disconnecting.

Connect now exposes password-saving consent, including profiles where it was
previously disabled. Successful secure storage commits the bound username and
consent to the saved session; reconnecting from a fresh tab can recall it.
Disabling saving removes the bound vault entry after authentication. Existing
master-password protection is retained when saving remains enabled.

Focused forms support Enter for their primary action; terminal/editor/chat input,
menus and IME composition retain their own behavior. The shared Windows UI font
baseline moves up one point, with font definitions cached across new terminals.

Desktop BGRA conversion now runs in a bounded background task with a single
latest-frame waiting slot. Prepared pixels use shared ownership for texture
updates; raw capture bytes remain unchanged. Input workers remain independent.
Idle surface refresh is reduced while each prepared frame wakes the renderer,
and large file panes draw visible rows only. These address verified source costs;
live latency and visual behavior still require a user build. Five new native
regressions are registered for the existing build-kit step 2. Source/offline
checks passed without compilation; acceptance flags remain unchanged. See
SESSION-UI-TASKS.md for the remaining verification work.

## UI and packaging follow-up - 2026-10-15

The latest session/dock/sidebar/AI layout work and cleanup status are tracked
in `SESSION-UI-TASKS.md` (U1-U10). The product executable is `AetherDesk.exe` with
the existing transparent Windows icon. Successful full and portable builds now
remove staging and compiler/download caches, retaining finished packages and
evidence; see the build-kit README for the cache-retention override. These
source changes do not complete R12 live/clean-machine acceptance.
