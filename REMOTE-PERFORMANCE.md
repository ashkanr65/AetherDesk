# Remote desktop performance: X2Go comparison

Review date: 2026-10-15. Rust source review, not a measured benchmark.

Wayland is a display-system choice, not a guarantee of faster remote delivery.
The amount of work sent through the capture, encoder, network, decoder and
presentation stages determines interactive latency.

## What X2Go does differently

The [X2Go FAQ](https://wiki.x2go.org/doku.php/doc:faq:start) describes repeated-image
caching, image-specific compression (including lossless PNG), and elimination of
X11 round trips. Its [nxagent/KDrive comparison](https://wiki.x2go.org/doku.php/wiki:advanced:x2gokdrive:start)
distinguishes optimized X drawing requests from image-based delivery and describes
cached image/scroll handling. These are distinct approaches: classic NX forwarding
is tied to X11 and cannot simply be switched on in a Wayland compositor.

For traditional EDA applications, text, menus, lines and repeated shapes can
benefit from reuse instead of repeatedly encoding the resulting desktop image.
That is an architectural explanation, not proof that X2Go wins every workload.

## What the current AetherDesk source actually does

| Stage | Source evidence | Consequence |
| --- | --- | --- |
| Host rendering | agent-wayland/src/compositor.rs uses Pixman and copies the captured BGRA canvas after damage | CPU rendering and full-image memory traffic scale with total monitor pixels |
| Encoding | agent-supervisor/src/supervisor.rs feeds the canvas to agent-encoder/src/pipeline.rs; BGRA is converted to NV12 and H264 | A small changed region still goes through a full-frame video pipeline; H264 can compress unchanged macroblocks but capture/conversion remain significant |
| Transport | agent-supervisor/src/server.rs emits EncodedFrame messages | Not an NX drawing-command or server-side cached-tile path |
| Other protocol support | TileUpdate/CacheStore/CacheReference and region classification support exist elsewhere | Presence of these types does not mean the live agent emits them; it currently does not |
| Client presentation | remote_frames.rs and remote_view.rs now suppress identical decoded images and apply safe partial texture uploads | Reduces client conversion/upload work for sparse changes; does not reduce encoded wire traffic |
| Display configuration | Geometry/properties are compared and debounced before sending | Local monitor polling does not continuously send display settings or restart the host output |

For the example 4800 x 1800 canvas, BGRA contains 34,560,000 bytes per image.
At 30 changing images per second, each full-image memory pass touches about
1.04 GB/s. This is calculated raw memory traffic, not measured network usage.
Several capture/conversion/copy stages can each incur such traffic.

NV12/H264 is not lossless RGB. Increasing lossy compression is therefore not an
EDA-safe substitute for eliminating unnecessary work. This update preserves
native-resolution and encoder quality/frame-rate defaults. The existing congestion
controller can still react to sustained congestion; it has not been disabled.
Exact duplicate suppression happens after decoding and does not discard encoded
reference frames. Partial texture uploads have a complete-frame fallback if a
frame was skipped, resized or a texture recreated.

## Virtuoso 2025 colour requirement

The user reports incorrect colours in X2Go with newer EDA applications, including
Cadence Virtuoso 2025. This is a reported compatibility problem, not a reproduced
cause. Do not assume that reusing X2Go's architecture solves it, or silently
change application colour maps, visual depth, GL rendering or colour profiles.

A future lossless UI path must preserve full RGB samples without chroma
subsampling. Validate at native 1:1 pixels using coloured one-pixel lines,
closely spaced coloured shapes, dark/light backgrounds, text, selection layers,
redraw, pan and zoom. Also compare an actual Virtuoso 2025 screen against a
host-side reference, covering both ordinary windows and accelerated graphics.
Synthetic patch tests prove only the tested stage; they cannot establish that
the application, capture, encoding and final display are correct end to end.

The current H264/NV12 video path remains a limitation for exact fine-colour
fidelity. It has not been represented as lossless or validated on Virtuoso 2025.
Cache identifiers must include pixel format and dimensions, be invalidated on
resize/reconnect, and fall back to a complete exact update on cache mismatch.

## Implemented in this update

- Partial client colour conversion and texture uploads, with immutable complete
  decoded-image snapshots and full fallback when patch dependencies do not match.
- No repaint/upload for byte-identical decoded images.
- One bounded preparation worker per desktop; no extra unbounded frame queue.
- Decoder output copies mapped pixels once instead of zero-filling then overwriting.
- Monitor configuration sent on actual geometry/property changes; metadata-only
  host updates avoid recreating the compositor framebuffer.

These changes need the user's native build and live EDA check. No measured speedup
or X2Go parity is claimed.

## Remaining work, in priority order

| Priority | Job | Acceptance evidence |
| --- | --- | --- |
| P1 | Measure single/all-monitor capture, encode, network, decode and presentation times under identical EDA tasks in AetherDesk and X2Go | Same host, client, network, resolution and application; p50/p95 input-to-visible latency, CPU and bytes/s, not just FPS |
| P2 | Implement a negotiated lossless changed-region/cache/scroll path for static EDA UI, with video only for motion where appropriate | Exact pixel tests for text, coloured wires, pan/zoom, cache misses/eviction, reconnect/resize and mixed updates; bound memory and provide full refresh fallback |
| P3 | Reduce host capture/colour conversion copies and investigate accelerated rendering/encoding where supported | Stage timing proves the actual bottleneck; preserve software fallback and exact UI path |
| P4 | Run installer/portable and mixed-DPI live acceptance on the completed pipeline | No input stalls, stale patches, colour bleed, monitor misalignment or unrelated-session regressions |

No remote-host commands, application builds, compilation or native benchmarks
were run for this review. Existing R12 replacement readiness remains incomplete.


## Keyboard failure investigation — 2026-10-16

This first investigation covers the confirmed terminal-pausing incident. The
all-application recurrence and client repair are recorded in the follow-up below.

Status: TERMINAL PAUSING CONFIRMED. User-selected prevention is implemented
in source; native/live acceptance is pending. The original report was that
mouse input worked but keyboard input failed in both tab and fullscreen. The
user then clarified that typing works in the remote editor but not the remote
terminal. Live testing below confirms editor delivery in tab mode. The user
subsequently confirmed Ctrl+Q restores terminal typing. This identifies paused
terminal output as the immediate cause. The prevention policy and fullscreen
shortcut acceptance require the user's new build. No keyboard-backend
replacement is applied. After the confirmed Ctrl+Q recovery, the user chose
terminal-level prevention while preserving other application shortcuts.

### Evidence and limits

- The running package is the 2026-10-15 19:36:14 portable package, not a newly
  compiled diagnostic binary. No application was restarted or rebuilt.
- The current client log had 54 records when inspected, and no keyboard-specific
  diagnostic records. It does not identify where keys stop.
- Both modes share remote_view.rs -> remote_keyboard.rs / GUI fallback ->
  remote_input.rs -> SSH input channel -> agent-supervisor -> Smithay keyboard
  seat -> the host-managed rootful Xwayland desktop -> the focused X11 app.
- Pointer and keyboard events share the input channel and supervisor state gate.
  Working mouse input makes a complete SSH-input-channel outage less likely;
  it does not prove keyboard capture or keyboard focus is working.
- Native capture readiness is a cached installation status. Windows can silently
  remove a slow low-level keyboard hook. AetherDesk can then still believe it
  owns keyboard capture and suppress GUI key events. This is a concrete design
  weakness and plausible explanation, not a reproduced cause on this PC.
- Host keyboard focus is a separate concern: forwarding a valid key into a seat
  without the intended focused surface may produce no visible text.
- Smithay 0.7.0 expects XKB keycodes internally and subtracts eight when sending
  Wayland keyboard events. The existing evdev +8 conversion matches that source;
  blindly removing it is not an evidence-based fix.
- UnicodeText is explicitly unsupported by the current host compositor. IME/text
  support needs separate handling; clipboard-only typing would not provide the
  physical shortcuts requested by the user.

Initial live inspection was interrupted by window activation failures. After
the user restored the window, fresh target selection and activation succeeded.
The actual AetherDesk remote desktop was verified visually before input. An
injected `a`, followed by `Shift+B`, appeared as `aB` in the unsaved remote
Mousepad document. No Enter, shell command, file save, EDA action or connection
lifecycle change was performed. The document was left open and unsaved.

This independently confirms ordinary and shifted key delivery to an X11 editor
through the running portable package in tab mode. It agrees with the user's
editor-versus-terminal observation. It does not establish that every physical
shortcut works, that fullscreen works, or that a terminal has received the same
events. The tool injects input; physical and injected input remain distinct.
The editor result and successful Ctrl+Q recovery do not support replacing the
keyboard backend for this incident. They do not prove that every shortcut or
presentation mode is free from unrelated or intermittent input faults.

Read-only SSH inspection was attempted separately rather than entering commands
in the existing terminal. The authorized `cadence` connection timed out, so no
remote process, PTY, terminal preference or focus query completed. No terminal
or host settings were modified. The user tried Ctrl+Q followed by letters and replied: "Typing works after
Ctrl+Q". This confirms the immediate recovery. No persistent terminal setting
was changed by the assistant.

### Terminal-specific checks and preventive choices

| Priority | Candidate | Smallest diagnostic / repair | Evidence still needed |
| --- | --- | --- | --- |
| 1 — confirmed | Output suspended / XON-XOFF | Ctrl+Q restored typing. User selected session-scoped prevention; implementation now clears IXON on matching desktop terminal PTYs, without filtering keys | Native Linux regressions and rebuilt-package acceptance pending; the exact original trigger was not observed |
| 2 | Terminal Read-Only mode | Inspect the affected terminal's Read-Only item; disable it only if enabled and the user chooses this repair | State of the actual installed terminal; current upstream Xfce source implements this mode |
| 3 | Terminal widget / input-method focus | Compare the affected window with a fresh disposable terminal. Trace active X11 window, focus and GTK/VTE input context if only the existing window fails | App focus and input events; do not assume that successful Mousepad input proves terminal focus |
| 4 | PTY / foreground job / shell echo state | Read process and PTY state through a separate authenticated channel. Repair only the identified terminal, preserving foreground work | Foreground process, terminal flags and whether input is accepted but not displayed; SSH is currently unavailable |
| 5 | Terminal-specific modifier or key translation | Compare letters, Shift, Ctrl and Alt in editor versus terminal and trace scancodes/modifier state without logging text | Reproducible failing chord or state; test both physical and injected keys |

Xfce's documentation describes Ctrl+S/Ctrl+Q terminal pausing. Linux termios
specifies IXON with VSTOP/VSTART. The Xfce source also includes a Read-Only
terminal action. Ctrl+Q recovery confirms paused output in this incident. Read-Only mode and
the other candidates were not established as active causes. Do not automatically send reset sequences, replace host
preferences, change all shells or kill the user's terminal to test them.

### Practical repair choices

| Choice | Change | When it applies | Trade-off |
| --- | --- | --- | --- |
| A — Diagnose, then repair the existing path (recommended) | Add short-lived counters for capture, GUI fallback, enqueue, agent receive, seat focus and app delivery; reproduce in a blank editor; fix the stage that loses input | First choice while the failing stage is unknown | Smallest architectural change; a diagnostic build may be needed because current logs have no stage counters |
| B — Make the current Windows capture fail over safely | Keep the existing hook for host shortcuts, detect disagreement with independent GUI/raw-input events, and stop discarding legitimate fallback keys when capture is unavailable | Client receives keys through one source but the current route suppresses them | Relatively contained change; source arbitration must prevent duplicate presses and release held keys on transitions; does not fix host focus |
| C — Windows Raw Input as primary capture | Use WM_INPUT for physical keyboard data, with a narrowly scoped hook only for shortcuts that Windows would otherwise consume | Windows capture is demonstrated to be the problem, or a larger client-side redesign is preferred | More robust device-level capture, but greater implementation/testing scope; Raw Input alone does not prevent Windows from handling local shortcuts and cannot repair host focus |
| D — Repair the existing Wayland/Xwayland focus/keymap path | Preserve/restore the correct seat and application focus during clicks, resizes, layout changes and presentation transitions | Keys arrive at the agent but do not reach the intended app | Retains current architecture and native Wayland support; only appropriate after host-side evidence |
| E — XTEST inside the isolated Xwayland display | Inject key press/release events directly into this session's authenticated X server | Agent receives keys and the X11 delivery path is the failing stage | Useful for the current X11 EDA desktop; must verify XTEST support and modifier/focus/grab behaviour. Not a native-Wayland solution and cannot recover keys never captured on Windows |
| F — Linux uinput device/backend | Connect a virtual keyboard to a deliberately isolated compositor input backend | A future compositor/device architecture needs it | Requires /dev/uinput access and actual libinput/device plumbing. Existing agent-input code is not currently the active private-seat route; enabling it alone will not fix this compositor. Host-wide device injection is unsuitable for isolated tabs |
| G — Alternative remote backend (RDP or VNC/RFB) | Replace the display/input backend with a mature protocol implementation | Strategic alternative if the custom backend cannot meet acceptance requirements | Largest change: host setup, packaging, authentication, lifecycle, monitor behaviour and EDA colour/performance acceptance must all be revisited; excessive for an undiagnosed keyboard-only failure |

Clipboard pasting, Unicode-only injection, dropping modifiers, and resending all
keys through both paths are not equivalent fixes: they break shortcuts, can
duplicate input, or miss the actual failure. Do not switch backend based solely
on the fact that the desktop uses Wayland.

### Recommended decision and acceptance

Ctrl+Q restored the affected terminal, and the user chose prevention. The new
terminal_flow_control.rs policy belongs to the hosted Xwayland desktop, runs
on a separate thread, and inspects same-user /proc entries every 500 ms. Only
numeric /dev/pts inputs whose process initial DISPLAY and private XAUTHORITY
both match this desktop are eligible. The optional screen-zero suffix (:N.0)
is accepted for the same X server; duplicate identity entries are rejected. The process start time and input target
are checked again after opening; other or inaccessible processes are skipped.
Only IXON is cleared, using TCSANOW without flushing data, and TCOON resumes
output when IXON is removed. Other termios flags and shortcut bytes are retained.
No host rc/preferences, keyboard backend or GUI shortcuts are changed.

The periodic check also catches terminals opened later and programs that restore
IXON. It can take a scan interval to correct a new or restored setting; it is not
a synchronous PTY-creation intercept. The guard stops/joins with its hosted
desktop and performs no further terminal changes after teardown. Unrelated
terminals with a different display or private authority are excluded.

Three registered Linux regressions exercise real PTYs, actual shortcut bytes,
resuming output, preserving other flags, a matching versus unrelated process,
re-applied IXON, guard shutdown and exact environment identities. These tests
have not been compiled or executed here. The existing build kit lists and runs
agent-wayland tests for both installer and portable packaging; no extra build
step or dependency is required. Verify using a newly built agent and a new
remote desktop session, since the currently running package is unchanged.

If a different failure remains, choose A for a traced failure that the terminal
checks do not resolve, B/C only for a confirmed Windows-capture fault, or D/E
only for a confirmed compositor/X11 delivery fault. Existing source tests do not replace a live end
to end reproduction. A diagnostic trace should count stages and focus/ownership
states without recording the user's typed text or API credentials.

Verify ordinary letters, numbers and punctuation, Ctrl+A/E/F/B/U/K/W/Y/T/_,
Alt+F/B/T, arrows, Home/End, F-keys, Alt+Tab, modifier release, repeat, keyboard
layout and AltGr in a disposable remote editor/terminal. Repeat in tab, single
monitor and all-monitor modes, after resize, after a popup and after changing
focus to local AI/files controls. Confirm that local controls receive no remote
keystrokes and that switching tabs never leaves modifiers held. IME needs its
own acceptance case. The explicit remote Ctrl+Alt+Delete control remains
necessary for the local Windows secure-attention sequence.

Primary references:

- [Microsoft: low-level keyboard hook lifecycle and timeout](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc)
- [Microsoft: Raw Input overview](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-raw-input)
- [Microsoft: using Raw Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/using-raw-input)
- [X.Org: XTEST key event injection](https://www.x.org/releases/X11R7.5/doc/man/man3/XTestFakeKeyEvent.3.html)
- [Pinned Smithay keyboard implementation](https://github.com/Smithay/smithay/blob/v0.7.0/src/input/keyboard/mod.rs)
- [Pinned Smithay Wayland keyboard delivery](https://github.com/Smithay/smithay/blob/v0.7.0/src/wayland/seat/keyboard.rs)

- [Xfce terminal usage: paused output and recovery](https://docs.xfce.org/apps/xfce4-terminal/usage)
- [Xfce terminal upstream Read-Only action](https://github.com/xfce-mirror/xfce4-terminal/blob/master/terminal/terminal-window.c)
- [Linux termios: IXON and VSTART/VSTOP](https://man7.org/linux/man-pages/man3/termios.3.html)

Source validation for this policy: Rust formatting/parser checks, regression
reference/migration checks and offline build-kit recipe fixtures passed. Cargo
is mocked by the recipe fixtures; no native Rust test or application was built.


## Follow-up: keyboard failure in both editor and terminal — 2026-10-16

Status: REOPENED. Source repairs implemented; rebuilt-client/native acceptance
pending. Ctrl+Q recovery established one terminal-pausing incident, not the cause
of every keyboard failure. The later editor-and-terminal failure is distinct.

### Live evidence

The user reported failure in both applications in the portable package
AetherDesk-Rust-0.1.2-build1_20261016_092853_782. Its build-info fingerprint is
b43a38f1ec2b0c3886ed46a8d6b401229b745701477a50cdbd043a2c3d9e9873.
The current client log was empty when inspected, so it provided no key-stage
counters or focus/hook evidence.

Window control activated the existing app. The unsaved remote Mousepad editor
was used for key checks, followed by Ctrl+Alt+Esc, a click on the local sidebar,
re-entry to the desktop, and maximizing Mousepad. The user then confirmed that
their physical keyboard worked. No reconnect, restart, shell input, file save,
EDA action or host configuration change was performed. This establishes recovery
after a focus/capture reset; it does not identify which internal stage originally
failed. Tool-injected mouse clicks follow a different path from physical clicks
because the existing mouse hook excludes injected events, so timing/results of
those clicks are not evidence of physical mouse failure.

### Client repairs

- The remote surface focus ID is now based on the desktop identity, independent
  of parent UI scopes as well as preceding controls.
- With the viewport active, an unpaused desktop reclaims otherwise unowned GUI
  focus. Existing local fields/dialogs and explicit Ctrl+Alt+Esc release retain
  ownership; the desktop does not steal their focus.
- A mapped GUI key arriving while native capture claims exclusivity now causes
  release of that stale route and immediate GUI handling of the event. Previously
  the event was discarded solely because raw_active remained true. Text/IME
  events are not replayed as extra keys; intentionally passed lock keys are not
  mistaken for capture failure.
- The hook's own message thread reinstalls the keyboard hook asynchronously
  after that observed failure. Until it is ready, GUI input remains enabled.
  If recovery fails, the hook worker cleans up, reports failure and leaves GUI
  fallback available. Its status cannot claim capture during recovery.
- The keyboard callback uses try_lock for the route instead of blocking behind
  the UI. A contended event falls through to the same GUI recovery path.
- Removed an unconditional early Ctrl+Shift+K consumer in main.rs. The existing
  navigation handler still implements the shortcut, with the remote-focus gate.
- Focus restoration and capture recovery/failure now produce diagnostic records
  without logging typed text, scancodes, file content or credentials.

Windows can silently remove a timed-out hook, as documented by Microsoft. That
remains a supported failure scenario addressed by this repair, not an observed
Windows hook removal on this PC. No Raw Input, XTEST, uinput, RDP or VNC backend
replacement was made. The Linux terminal IXON policy remains separate.

### Remaining verification

The existing focus regression now varies parent UI scopes. Two new registered
Windows regressions cover window activation/local-field/explicit-release focus
ownership and GUI key recovery classification without replaying Text/IME events.
They are registered but have not been compiled or run in this review. Source
formatting/reference/migration checks and offline recipe results are recorded
in SESSION-UI-TASKS.md; recipe Cargo calls are mocked.

After rebuilding the Windows client, verify editor and terminal typing in tab,
single-monitor and all-monitor modes, then repeat after Alt+Tab, local AI/files
popups, resizing, sidebar changes, toolbar movement and return-to-tab. Verify
Ctrl/Alt shortcuts, modifier release and explicit Ctrl+Alt+Esc. Check that local
AI/settings fields receive only local input. Existing sessions need a new client
process to use this client-side repair; the investigation has not modified the
currently running executable. If the issue recurs, use the new recovery records
alongside a focused reproduction; do not declare end-to-end keyboard readiness
from source checks or the one successful live recovery.
