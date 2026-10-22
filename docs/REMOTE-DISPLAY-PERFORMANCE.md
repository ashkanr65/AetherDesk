# Remote display performance

Build 8 introduces exact changed-tile delivery for the native desktop client.
It targets large desktops used for schematics, layout editors, terminals and
other mostly static applications. Resolution is unchanged. Compression is
lossless BGRA; the new path does not convert these pixels through NV12/H.264.

The investigation found that software copy/conversion/H.264 processing alone
fell from about 30 to 8 frames/s when moving from a 1986x1170 tab to a
4800x1800 combined desktop. These are isolated pipeline results, not measured
input-to-display latency or results from the new implementation.

## Delivery

- The client explicitly opts in through the `ADLT0001` hello nonce prefix.
  The v3 envelope and old message layouts remain unchanged; new display variants
  are appended. The remaining 192 nonce bits stay random. Authentication still
  uses the independent random 256-bit channel token and authenticated SSH.
  Older agents ignore the prefix and keep their video path. Other consumers
  must not opt in unless they own a lossless assembler.
- The host compares complete images against the last sent image on a 256-pixel
  grid. It cannot lose damage when capture skips intermediate frames. Each tile
  is sent as bounded LZ4 or raw BGRA, whichever is smaller.
- One remote desktop and its monitor layout are retained. Only changed tiles
  are sent, so an idle monitor needs no tile payload. Tiles entirely outside
  every monitor are skipped. Tiles crossing monitor/gap edges can include a
  small part of the unused canvas. Overlapping monitors never duplicate tiles.
- Begin, tiles and commit travel in order on the media channel. The client
  validates the base, bounds, grid, count and decompressed lengths before
  publishing a complete transaction. A receipt acknowledges that commit after
  application to the framebuffer. Frame/byte credits bound the backlog.
- Reconnection, size/layout changes and refresh requests send all visible tiles
  from a fresh base. The receiver clears the old canvas on that full refresh.
  Malformed/incomplete transactions fail explicitly instead of showing a
  partial desktop. The SSH input channel remains separate from media writing.
- Client snapshots remain complete and immutable for screenshot attachments.
  Validated damage avoids a full pixel scan only when preparation has the exact
  base image. After a skipped preparation/upload, the complete image remains
  available for recovery.

The host's render/capture cadence follows the sender's frame-rate controller.
The compatible video path wraps packed captures with their immutable owner
instead of copying them again; padded rows keep the row-copy fallback. Its
colour converter can use up to four workers. Lossless delivery adapts cadence,
not colour values or resolution, under congestion.

This change does not enable unverified hardware decoding or introduce multiple
H.264 decoder instances. Existing software decoding remains the video fallback.
The lossless tiled path has no 4096-pixel H.264 hardware decoder limit. Exact
updates can use more bandwidth than lossy video for full-screen photographic
motion; measure that workload separately. For diagnostic comparison only,
starting the client with `AETHERDESK_DESKTOP_VIDEO=1` disables tile opt-in.

## Fullscreen

External desktop windows are registered once each root pass for every open
connection. They remain visible when another connection is selected in the
main window. Return to tab/native close selects the correct connection without
ending its desktop. Disconnect and End remain separate actions. Native input
capture still requires the exact foreground window.

Immediate viewports continue to share repaint work with their root. Independent
repaint ownership is a possible follow-up after the new pipeline is measured.
The toolbar's floating auto-reveal area also needs ordinary user acceptance;
its intentional reveal behavior was not classified as a confirmed input bug.

## Verification before release

Source/parser and offline build-kit checks are separate from native execution.
The required registry includes bounded codec, atomic assembly, idle monitor,
skipped capture, large canvas, padded row, real in-memory wire/backpressure,
texture recovery and actual egui viewport registration fixtures. The retained
desktop fixture also exercises legacy delivery followed by lossless resume.

After the user builds, compare the same EDA workload at unchanged resolution in
tab, single-monitor fullscreen and all-monitor mode. Check exact colour patches,
thin coloured lines, transient/final dialogs, idle final updates, resize/reconnect
refreshes, mouse/keyboard routing and switching the main window to SSH+X11.
Capture client/host CPU, memory, payload rate and displayed cadence under that
same workload. No new speedup figure is established until this acceptance runs.

Numeric logs report host preparation/write time, receipt duration and pending
frames/bytes, plus client application/texture preparation time. They contain no
pixels or input contents. Receipt duration includes delivery and client apply;
it is not an input-to-visible latency measurement. Idle periods must not be
mistaken for a low frame-rate bottleneck.

Retained agents keep their running binary and compatible video behavior.
Use a newly created desktop with the rebuilt bundled agent to test the new host
path; do not terminate an existing desktop with unsaved applications for testing.


## Selected application window prototype (local 0.1.3 build 1)

This is local development work under an explicit publication hold. Build 8
remains the public release. No source or package should be pushed/published
until the complete feature is finished and the user authorizes publication.

The remote desktop toolbar has a Remote applications icon. It opens a native,
searchable chooser that can be used alongside tab, fullscreen and all-monitor
views. Select one existing visible X11 application to show an independent
Windows view. The original process, X display, WM frame and desktop
representation remain in place; closing the view stops viewing only.

A separate `ad-agent --window-mirror --session-id UUID` helper uses an exec
channel inside the same SSH/VPN connection. It authenticates only to that
saved desktop's private Xauthority/socket, opens no TCP listener and never
accepts an arbitrary display or identity path from the client. Older agents
without the helper fail this optional operation without restarting a desktop.

Only the selected window is redirected using Composite automatic mode. The
helper does not enable desktop-wide effects or replace the WM. DAMAGE events
trigger bounded captures; remapping and resizing recreate the backing pixmap
and force a complete refresh. Lossless BGRA/LZ4 tiles retain exact RGB values,
with atomic client validation and one request/response frame in flight.
Unsupported colour visuals and oversized windows fail explicitly. The
prototype limits one image to 8,388,608 pixels and one packet to 36 MiB.

Clicking the view requests activation of the original server window. Ordinary
physical keys/modifiers, pointer buttons/wheel and explicit resize are
supported in source. Input requires that window to own server focus; parts
outside the server screen are rejected. Focus loss releases helper-owned
input. EOF, cancellation and bounded pipe read/write leases retire helper
resources without terminating the application. The stdin lease uses
nonblocking descriptors rather than Tokio's uncancellable blocking stdin.

This is an independent mirror, not complete seamless application remoting.
Managed dialog families, multiple views, native keyboard/IME and
clipboard integration, complete mixed-DPI acceptance, reconnect restoration,
hiding the original safely and native Wayland application capture remain
future work. Client conversion/upload currently uses complete images; do
not claim measured latency, bandwidth or CPU improvements before profiling.

Static checks and mocked build orchestration pass. Native tests are registered
but have not been compiled or executed. The isolated real-Xfwm fixture now
covers occlusion, exact colours, damage, resize, owned-key release and keeping
the original window/WM parent alive. The user's next local build must run
those tests before a disposable application test, then explicitly scoped
Cadence compatibility and performance acceptance. Existing unsaved work and
Cadence libraries must remain untouched.

## Fullscreen to all-monitor crash correction (local 0.1.3 build 1)

The 2026-10-21 local crash records for processes 2908 and 29744 both end in a Rust panic in the wgpu fatal-error handler. The matching application logs identify the DX12 Intel Arc client renderer. The logs show single-monitor and all-monitor frame sizes around the transitions, but do not retain the actual wgpu validation message. This establishes a client rendering failure, not a confirmed server, SSH or GPU-capacity failure.

Source inspection found native SetWindowPos calls inside the immediate viewport drawing callback, before eframe submits/presents that frame. The correction applies the validated physical bounds only after the immediate renderer returns, leaving eframe to process the resulting window-resize event. A toolbar click selecting another mode cannot apply unvalidated new bounds during the current frame. This preserves the physical monitor layout and remote session.

Frame resizing now uses a fresh texture identity; partial uploads require matching texture dimensions. Both incoming remote frames and external presentation bounds are checked against the renderer limit before GPU allocation or native resize. DX12 requests the adapter-supported 2D limit up to the protocol ceiling of 16384 rather than always requesting egui's default 8192. Unsupported arrangements return to the tab with an error instead of reaching a fatal GPU operation. These checks do not silently reduce colour precision or remote resolution.

Fatal graphics validation details are now redacted, bounded and written synchronously to the crash log before retaining fatal-error behavior. This does not suppress device errors or pretend recovery succeeded.

Regression source extends the immediate-viewport fixture with repeated tab/current/all transitions, negative monitor origins, mixed DPI, post-render native placement and rejection before unsupported placement. Texture tests cover repeated dimension changes, full uploads after resize and preservation of the last valid frame when a texture is too large. Graphics diagnostic tests cover redaction and bounded size. These tests are registered for the user's Windows build; they have not been compiled or executed here. Formatting and registry/reference checks are separate static checks. Actual DX12 acceptance on the reported monitor sequence remains pending.

Relevant renderer source: egui-wgpu 0.33.3 winit painter and eframe 0.33.3 native wgpu integration, inspected from the build cache. Upstream reference: https://github.com/emilk/egui/tree/0.33.3/crates/egui-wgpu/src .

## Independent-window input-release acknowledgement (local 0.1.3 build 1)

The user's Linux packaging run on 2026-10-21 reached the real host desktop fixture and failed its immediate key-up assertion: keycode 38 was still set (bit value 64). The mirror sent unchecked XTest release requests, cleared its ownership sets, and merely flushed the socket. A query through the fixture's other X11 connection could be processed before those releases. Cleanup errors were also discarded.

The correction checks each owned key/button release, attempts the remaining releases even after an error, retains failed entries for retry, and performs an explicit same-connection X11 server round trip before acknowledging Release or Stop. Focus transitions propagate release errors. Ordinary matching key/button release retains ownership until checked success. Drop tries cleanup and reports failure without panicking. No global key reset is used.

The real fixture now repeats immediate cross-connection key/button checks eight times, checks idempotent release, ordinary releases, Stop and Drop, and holds an unrelated key on the controller connection to verify it remains untouched. The input assertions contain no sleeps or retries. This extends the already registered host_xfwm_draws_decorations_and_handles_input_and_resize test. Source formatting and catalog/reference checks pass; the Linux test has not been rerun by the agent under the retained user-only compilation instruction.

Protocol reference: XTest requests preserve same-client processing order through simulated event completion (https://xorg.freedesktop.org/archive/X11R7.5/doc/Xext/xtest.html). The pinned x11rb 0.13.2 wrapper sync method uses GetInputFocus to wait for preceding requests to finish; flush alone provides no such processing acknowledgement.


## Application sizing, menus and keypad corrections (local 0.1.3 build 1)

The independent view starts with one remote pixel per physical Windows client pixel, with the selected application size plus the local toolbar and monitor work-area limits. It does not stretch a small application to fill a larger view. Native resize changes are debounced before asking the server WM to resize the original application; server-side resize refusal remains visible rather than being disguised by image magnification.

The optional mirror protocol is ADWIN002. It adds bounded menu layers with signed offsets, exact BGRA/LZ4 tiles and atomic validation alongside the unchanged application dimensions. Visible override-redirect windows must belong to the selected application's transient family or match its client/leader/PID and menu type with adjacency. For legacy Xt/Motif menus lacking EWMH properties, XRes QueryClients supplies the actual selected client's resource range; only adjacent untyped shells from that range while the application is active are accepted. If XRes is unavailable, this fallback is unavailable; explicit ownership remains supported. Normal managed dialogs and arbitrary desktop windows are not included automatically.

Each menu is an undecorated, non-activating-at-creation native viewport owned by the independent application view, with physical placement after rendering. This allows a dropdown beyond the original client edge without enlarging its image. Main and menu input share focus/held-key cleanup; popup-relative pointer coordinates remain signed and are accepted only inside the application or its known menus. Closing a menu view requests Escape; stopping the independent view only releases capture/input resources, preserving original windows. Menu-only changes reuse the main image and texture. The combined application/menu pixel and packet bounds prevent unbounded capture.

Numpad operators previously lost physical identity on the egui fallback route: add/divide/subtract become ordinary punctuation and Multiply has no matching egui key. Dedicated evdev codes now survive both native routing and a foreground-window-only GUI message hook. This hook records bounded keypad metadata on the GUI thread, not global text, and is disarmed with remote input ownership. The real compositor fixture checks dedicated keypad operator symbols. Upstream mapping: https://github.com/emilk/egui/blob/0.33.3/crates/egui-winit/src/lib.rs .

Regression source covers one-to-one physical image sizing, bounded menu metadata, nested native viewport registration/cleanup, real Xfwm menu colours, beyond-edge input, untyped same-client menus and exclusion of another client's popup. These tests have not been compiled or run here. Actual Cadence dropdowns, colours, managed dialogs, IME, clipboard and mixed-monitor behavior still require controlled acceptance before declaring seamless remoting complete.

## Active-host status clock (local 0.1.3 build 1)

The bottom-right display now comes from the selected connection's server timezone. One non-PTY exec channel on that tab's existing SSH/VPN session answers one small clock pulse per second with `date +%H:%M:%S`. The client displays only a validated server reply; it never advances a cached time with its local clock. A missing initial response, disconnection, remote-client failure or three seconds without a fresh reply displays `--:--:--`. Changing tabs chooses that connection's independent clock state.

The command waits for client pulses rather than leaving a sleeping or detached remote process. EOF/cancellation closes the channel and stops its read loop. Failed requests clear the time and retry on the same connection with bounded timeouts. No shell commands enter interactive history. This is a lightweight SSH-response indicator; it cannot establish that every server application, encoder or display path is functioning. Tests for parsing, freshness, disconnect and channel lifetime are registered but await the user build.


## Shift, legacy filenames and idle/wake review (local 0.1.3 build 1)

The supplied core-16396 log establishes successful SSH authentication followed by several early desktop-stream EOFs and two SFTP listing failures on non-UTF-8 filenames. It does not establish a network, SSH authentication, GPU or server-agent root cause for the EOF. The supplied core-21092 log has 947 capture-loss/recovery pairs and no fatal record. The user reports Windows closed the unresponsive application after idle, with no companion crash log; an application hang is possible, but a stack or Windows Application Hang report is still needed to establish the blocking operation.

Modifier snapshot handling previously could emit a left-Shift/Control/Alt/Super edge through synchronization and then emit the same physical edge again. The Wayland and uinput paths now suppress only that already-applied edge, preserving right-side physical keys and ordinary keys. GUI fallback reconciles Windows modifier state at batch boundaries because native capture can suppress the messages egui otherwise uses to track Shift. Detached views reconcile after their event batch and release modifier-only/button-only ownership. This addresses source hazards; it does not substitute for physical keyboard acceptance on the affected computer. Windows input reference: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getasynckeystate .

Capture recovery now has a bounded retry policy while GUI forwarding remains active: 1-second cooldowns for initial failures, then 30 seconds for repeated failures in a 30-second window. It no longer reinstalls native keyboard hooks at display-frame frequency. OS-intercepted shortcuts may be handled locally during GUI fallback. Heartbeats skip sleep-time backlog and bound writes, rather than flooding the agent on wake or indefinitely blocking the control pump on a stalled send.

Native clipboard access previously ran on the GUI thread every 300 milliseconds. It now runs on one shared worker with an eight-job queue and nonblocking GUI polling. Requests and results retain a 1 MiB text limit; per-view outgoing writes coalesce, failed writes remain queued rather than echoing stale local content, and retired views cancel queued jobs. If an OS clipboard call never returns, synchronization can stop but painting/input does not wait for it, and no replacement workers accumulate. A controlled stalled-backend regression verifies nonblocking submission, bounded queueing and cancellation. The log does not prove that clipboard access caused the reported hang.

A directory containing a non-UTF-8 filename no longer disables SFTP as a whole. Such entries are visibly disabled with an escaped byte identifier and no actionable path. Other names retain exact bytes and remain usable. Transfers and recursive removal reject unsupported entries, rather than inventing lossy paths or omitting files silently. Full byte-path editing/transfer support is a separate limitation; no server filename is changed.

Failures now preserve the transport error recorded before a demuxer closes its consumer channels and identify which SSH channel ended. Background workers stop before lifecycle cleanup. On failure the client attempts a bounded, redacted tail of only the current session's agent log through the existing connection. No server application is restarted for diagnosis. These additions improve future evidence; they do not retroactively explain the early EOFs in the supplied log.

Regression source and static checks are separate from compiled/live evidence. All these changes await the user build, real mixed-encoding listing, physical Shift and repeated idle/lock/sleep/wake checks. Existing server applications, unsaved work and Cadence libraries were not touched.
