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
