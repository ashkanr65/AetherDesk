# Notification, Tab Overflow, and NX Root-Cause Report

Date: 2026-08-27

## Scope and outcome

The notification lifecycle and tab-overflow defects are fixed and covered by
regression tests. The NX screenshot is fully diagnosed but is **not** falsely
reported as fixed: the repository has no NX client or renderer. A complete
standalone implementation and its migration plan are defined in
`WAYLAND_X11_REMOTE_DISPLAY_EVALUATION.md`.

## Issue 1 — notifications did not auto-dismiss

### Root cause

The deletion notice contains an Undo action. In the Flutter version pinned by
the project (3.47.1), the `SnackBar` constructor derives `persist` as
`action != null` unless explicitly overridden. `ScaffoldMessenger` does create a
duration timer, but its timer callback returns without hiding a persistent
snackbar. The duration was therefore ignored for exactly the notice shown in the
screenshot.

Two related lifecycle defects existed:

1. `AetherNotice.show()` called `hideCurrentSnackBar()` before every new notice,
   so bursts replaced the visible item rather than forming a stable queue.
2. The inline banner timer was created during `initState`, before the first
   visible frame, so its countdown did not satisfy “start when visible”.

This explains why previous duration-only changes did not work. Changing five to
three or eight seconds cannot affect a snackbar that Flutter has made persistent.

### Permanent fix

`AetherNoticeHost` now owns notification visibility and lifetime instead of
delegating product behavior to implicit `SnackBar` rules.

- A notice becomes current, renders, then starts its five-second timer from a
  post-frame callback.
- The final 180 ms of that lifetime uses a non-blocking opacity animation; a
  separate expiry timer still removes the widget at exactly five seconds.
- The timer removes the current entry and atomically advances a FIFO queue.
- Every timer is cancelled and protected by a generation/id check when an entry
  changes or the host is disposed.
- The X remains available for immediate dismissal.
- An action runs once and dismisses its matching entry even if the callback
  throws.
- The pending queue is bounded at 256 entries to prevent unbounded memory growth
  under a notification storm. A 100-entry burst is preserved in full.
- The fallback `SnackBar` path explicitly sets `persist: false` for embedders
  that omit the host.
- Inline banners also begin timing after their visible frame and cancel stale
  fade/expiry callbacks on update/disposal.

### Verification

Automated tests verify:

- status, information, warning, and error policy is five seconds;
- an actionable Undo notice exists at 4,999 ms and is absent at 5,000 ms;
- fade-out begins 180 ms before expiry without extending the lifetime;
- three notices display and disappear in FIFO order;
- 100 queued notifications drain without stale entries;
- disposed inline notices cannot call a stale timer; and
- source analysis has no errors or information-level findings.

## Issue 2 — hidden session tabs

### Root cause

The old horizontal `ReorderableListView` could technically scroll, but the
application had no `ScrollController`, overflow detection, left/right controls,
or logic to reveal the selected tab. Mouse users had no discoverable way to
reach an off-screen tab, and selecting/opening a hidden tab did not move the
viewport.

A second state bug existed: editing a saved connection iterated over every open
tab with the same profile id and replaced its profile. A running tab could
therefore change host, connection mode, or desktop label after it had opened.

### Permanent fix

`SessionTabStrip<T>` now provides:

- automatic overflow detection from real scroll metrics;
- left/right navigation buttons only when overflow exists;
- enabled/disabled arrow state based on scroll position;
- smooth 180 ms page scrolling;
- automatic selected-tab reveal after selection, opening, reorder, resize, or
  tab-count changes;
- Ctrl+Tab and Ctrl+PageDown for next tab;
- Ctrl+Shift+Tab and Ctrl+PageUp for previous tab;
- wraparound keyboard navigation;
- drag reordering; and
- immutable per-tab connection snapshots: editing/deleting a saved profile no
  longer mutates existing sessions; and
- a fixed item extent and lazy builder/cache, making selected-index offset
  calculation O(1) and avoiding construction of all 100+ tab widgets.

### Verification

The widget regression test creates 100 tabs, confirms that the list remains
lazy, confirms both navigation controls, scrolls right, confirms backward
navigation becomes available, selects tab 100, and verifies its rectangle is
inside the viewport. Unit tests verify keyboard wraparound and that new sessions
append without replacing existing tabs.

## Issue 3 — screenshot UI/NX failure

### Notification portion

The bottom deletion notice in the screenshot is the exact actionable snackbar
case fixed in Issue 1.

### NX portion: root cause

The server is healthy. The remote probe finds an X2Go/NX session service and the
selected XFCE launcher. AetherDesk then has nothing capable of consuming that
protocol:

- `ad-display`, `ad-stream-client`, `ad-x11-bridge`, and `ad-rdp-bridge` are
  generated stubs;
- `ad_rdp_connect()` always returns “RDP engine is not implemented”;
- the Flutter remote-desktop launcher only tunnels to xrdp and launches the
  external Windows `mstsc.exe`; and
- the X11 launcher requires external VcXsrv.

NX, RDP, X11, and Wayland are not wire-compatible. XFCE selection is a desktop
startup choice, not a protocol converter. Retrying, changing timeouts, removing
the readiness probe, or claiming xrdp is missing would all be temporary or false
workarounds.

### Correct status and permanent solution

The current code correctly classifies an NX-only server as a missing AetherDesk
client engine and writes a credential-free `remote.probe` audit event. This
diagnostic prevents destructive server-side advice but does not implement NX.

The acceptance criterion “the NX issue cannot be reproduced” is therefore not
met by this patch, and no release should claim otherwise. The permanent work is:

1. ship an embedded, signed NX-compatible Windows client/renderer for the
   existing XFCE service; then
2. ship the signed AetherDesk Linux agent and hybrid Wayland/X11 streaming engine;
3. replace `mstsc.exe` with embedded FreeRDP/IronRDP for optional xrdp hosts; and
4. remove the VcXsrv dependency through the embedded renderer/agent path.

The architecture, security boundaries, benchmark matrix, risks, and estimates
are specified in `WAYLAND_X11_REMOTE_DISPLAY_EVALUATION.md`.

## Welcome page and capability truthfulness

The previous welcome page reduced the product to generic capability chips and
labelled Remote Desktop “ready” from a single Boolean even though the native
RDP and NX renderers are absent. The native capability schema now separates:

- an in-process remote desktop renderer;
- NX remote desktop support;
- Windows RDP compatibility; and
- bundled versus compatibility X11 support.

The redesigned welcome page presents the four actual workflows, independent-tab
behavior, security posture, and an explicit `Compatibility` state for the RDP
launcher. The connection screen still permits that supported path without
claiming the placeholder Rust bridge is implemented.

## Documentation cleanup

Fifteen duplicate Windows/build/status documents were removed. Several claimed
that RDP and VNC were complete while the corresponding native functions always
returned not-implemented errors. `BUILD_WINDOWS.md` is now the canonical build
and packaging guide, and `core/rust/README.md` records the implemented and
scaffolded boundaries.

## Files modified in this repair

- `ui/flutter/ad_remote_desktop/lib/notifications.dart`
- `ui/flutter/ad_remote_desktop/lib/session_tabs.dart`
- `ui/flutter/ad_remote_desktop/lib/app.dart`
- `ui/flutter/ad_remote_desktop/lib/native_bridge.dart`
- `ui/flutter/ad_remote_desktop/lib/remote_desktop.dart`
- `ui/flutter/ad_remote_desktop/lib/session_store.dart`
- `core/rust/crates/ad-ffi/src/bridge.rs`
- `ui/flutter/ad_remote_desktop/test/notifications_test.dart`
- `ui/flutter/ad_remote_desktop/test/capabilities_test.dart`
- `ui/flutter/ad_remote_desktop/test/remote_desktop_test.dart`
- `ui/flutter/ad_remote_desktop/test/session_tabs_test.dart`
- `BUILD_WINDOWS.md`
- `core/rust/README.md`
- `REMOTE_DESKTOP_PROTOCOL_DECISION.md`
- `WAYLAND_X11_REMOTE_DISPLAY_EVALUATION.md`
- `ROOT_CAUSE_VALIDATION_REPORT.md`

## Validation results

- Dart formatting: passed.
- Flutter/Dart analysis with fatal infos and existing dependency metadata:
  passed, no issues.
- Full Flutter regression suite: 19 tests passed, including exact expiry,
  fade, 100-notification, 100-tab, capability, and desktop-profile coverage.
- Rust tests were not run locally because Cargo is not installed in this
  Windows environment; the GitHub workflow remains configured to run the
  locked `ad-ssh` and `ad-ffi` suites before packaging.
- Windows release build: GitHub remains the authoritative clean build because
  local Windows Developer Mode is disabled and Flutter cannot regenerate plugin
  symlinks locally.

## Remaining limitations and risks

- NX remote desktop remains a major unimplemented product subsystem.
- RDP compatibility still depends on the Windows inbox client and server-side
  xrdp/xorgxrdp.
- X11 mode still depends on VcXsrv.
- A 257th pending notification drops the oldest pending (never the visible)
  entry to keep memory bounded. If every event must be retained, durable events
  belong in the audit/event log rather than a transient UI queue.
- Tab bodies remain retained by the existing `IndexedStack` to preserve live
  session state. One hundred real terminal/desktop sessions will consume the
  resources of those sessions even though the tab header list itself is lazy.
