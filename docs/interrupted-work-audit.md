# Interrupted MiMo work: repair audit

Date: 2026-10-08. Source: `AetherDesk`, alongside `AetherDesk-Beginner-Build-Kit`.
The four review notes were checked against the actual working tree. No application,
installer, Linux agent or native test executable was built during this repair.

## Findings and changes

| Priority | Area | Interrupted implementation / defect | Repair and verification |
|---|---|---|---|
| P0 | Remote screen stalls | Network acknowledgements were removed but the display consumer never sent replacements. The host could stop after its delivery window filled. | Both display consumers now acknowledge decoded/applied frames. Recovery releases outstanding dependencies so a replacement keyframe can flow. Delivery-window and receiver regressions remain in the normal Rust test suites. This is framebuffer application, not a measured physical display presentation time. |
| P0 | Repeated SSH connections | Dropping a raw SSH channel does not necessarily send CLOSE. Timed-out commands, cancelled setup and X11 anchors could retain remote resources. | A channel ownership guard closes abandoned command/shell/SFTP/LSP setup channels. Successful SFTP streams retain their existing close-on-drop behavior. Exec cancellation regression added to the in-process SSH server test. |
| P0 | Disconnect cleanup | File-operation and language-server registrations survived removal of their owning SSH connection, consuming global limits. | Registrations carry session ownership; disconnect cancels and removes only that session's work. Late registration checks prevent a disconnect race. Registry regression added. Both SSH transports cancel together; application shutdown runs outside the Flutter UI isolate. |
| P0 | X11 helper lifetime | Watching the helper's immediate parent is unreliable after shell backgrounding/reparenting. | The SSH anchor explicitly supplies its PID. Linux process start time protects against PID reuse. Compositor-owned helpers without that marker retain their existing lifecycle. Three new Python lifecycle tests pass. |
| P0 | Unsafe cleanup | MiMo added automatic deletion/termination of retained desktops beyond a fixed count. This could kill intentionally retained work. | Removed this policy and its unused status files. Normal disconnect retains a remote desktop; explicit End remote session remains the supported termination action. Existing orphan processes from old versions are not indiscriminately killed. |
| P1 | Monitor changes | One/all-monitor snapshot mismatch could repeatedly refresh; resolving a toolbar reference after destroying its window selected the wrong monitor. Rebuild failure could leave unusable views. | Resolve references before teardown; compare global snapshots; debounce display/DPI/settings events and poll arrangement changes. Prepare replacement views before switching. A failed host configuration preserves the previous view. Single-monitor mode follows main-window monitor changes and surviving monitors. |
| P1 | Movable fullscreen toolbar | MiMo's dragging and auto-hide/input interactions were incomplete. | Hamburger dragging supports floating placement and snapping horizontally to top/bottom or vertically to left/right. Dragging keeps the menu visible, releases held remote input, and bounds controls to the client area. Remote button releases over the toolbar are preserved. Native tests cover all edges and failure cases. |
| P1 | Logging | Rust, native and Dart writers competed over one file and rotation. Writer queues were unbounded; context could change before asynchronous writes. | Independent producer files, bounded Dart/Rust queues, merged recent history, redaction, stacks, source/severity/search filters and export. Host IDs are validated and fetches cannot overlap or overwrite a newly selected host. Audit copy/export now matches the displayed audit records. See `logging.md` for limits. |
| P1 | Responsiveness | Several useful MiMo changes were incomplete or needed safeguards. | Retained bounded frame/byte windows, deeper bounded queues, sustained-congestion gating, status equality, terminal buffer/theme reuse, idle poll backoff, pointer coalescing and editor deltas. File hashing caches quiet polls but rechecks stable content before upload and periodically checks unchanged metadata. Retained x264 ultrafast to avoid an unmeasured CPU regression; quality has a quantization bound. |
| P1 | Build/test layout | Generated native test output was in the source root. Source tests themselves already belonged to their components. | Removed only the verified untracked `native-desktop-tests` output. Native test builds now use ignored `build/native-desktop-tests`; snapshots exclude the old location. Shared Windows recipe checks SSH/FFI formatting and invokes lifecycle tests. The sibling kit already delegates to these source recipes, so no duplicate tests were copied into it. |

## Validation

- Final Flutter suite after all UI changes: 190 passed, 2 skipped.
- Flutter static analysis: clean.
- Shared `Test-Recipe.ps1`: passed. Compiler/build commands in this recipe test are mocked; no build was performed.
- Python checks called by that recipe: 10 assistant packaging tests passed; 10 X11 tests passed and 1 Linux-only check skipped on Windows.
- Rust formatting checks passed for SSH/FFI and all packages selected by the Linux recipe.
- New native and Rust regressions were added but not compiled or executed, honoring the no-build request. They run in the kit's NativeTests/TestCore/Linux checks stages.

After the manual build, verify real Windows mixed-DPI hotplug/rearrangement,
all four toolbar docking edges, switching one/all/tab modes, repeated SSH/SFTP/X11
connections, and a long-running video session on the target Linux host. Automated
source/UI checks cannot establish GPU behavior, network latency or host process
cleanup on that hardware.

## Larger proposals still pending

The speed notes also propose new subsystems: hybrid lossless damage tiles/video,
glyph and persistent cross-session caches, QUIC transport, NX/X2Go/FreeRDP engines,
D3D11 zero-copy textures, predictive cursors, a full event-driven terminal/decoder,
per-frame end-to-end telemetry, and network/hardware benchmark matrices. Those are
not implemented by this repair and no speedup percentage or X2Go equivalence is
claimed. The existing separate bulk SSH transport was already present before MiMo;
the notes' claim that remote video always shares terminal transport was stale.

## Cleanup requiring approval

`core/rust/target` contains generated files that are nevertheless tracked in Git.
Automatic approval review rejected deleting that whole tree because it includes
tracked files. It has been preserved. Removing it from the source repository is
a separate cleanup requiring explicit approval; the kit already excludes it from
source snapshots. The four input notes and `.freebuff` history were also preserved.


## Build follow-up: native toolbar test (2026-10-08)

Step 6's `desktop_windows` failure (0xc0000409) was an assertion abort,
confirmed in `Testing/Temporary/LastTest.log`. The test incorrectly required
a side-docked toolbar to be taller than it was wide. Its intended geometry
at 96 DPI is 180 x 152 pixels with three buttons stacked below the handle.
Replaced that aspect-ratio assertion with checks of the actual child windows:
full-width, ordered, contiguous, non-overlapping rows below the handle,
contained within the toolbar and viewport. The same checks cover left and
right docking. The native test remains enabled in the build kit.
No native compilation was performed during this follow-up; the updated native
test must run during the user's next manual build.

## Verified Windows follow-up: scaled-monitor snapshots (2026-10-08)

The next Step 6 log progressed past toolbar docking and failed the stable-monitor
assertion. The test created its monitor snapshot in the caller's DPI context,
while CheckMonitorSnapshot used physical pixels. Scaled displays therefore
appeared to have changed. EnumerateMonitors now establishes physical-pixel DPI
awareness itself and restores the caller's context. A regression explicitly
compares snapshots from DPI-unaware and per-monitor-aware callers and checks
that an unchanged monitor arrangement does not schedule a refresh.

The user explicitly authorized compiling only the Windows regression tests.
Built the three test targets from the corrected source with MSVC Release and
ran CTest: desktop_platform, desktop_windows and stream_surface_shutdown all
passed (3/3, zero failures). The editor shutdown test includes 100 child cycles;
the desktop test includes its GDI resource stability check. This supersedes the
earlier native-test validation limitation. No application, Linux agent, installer
or portable package was built. Test artifacts stay in ignored build/native-desktop-tests.