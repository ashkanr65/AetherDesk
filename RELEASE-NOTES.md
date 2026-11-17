# AetherDesk 0.1.3 - build 10

Build 10 improves recovery after a monitor is disconnected, including normal window sizes saved by earlier versions.

## Changes in build 10

- Follow the active Windows keyboard layout in each focused independent application or menu, including applications detached from Remote Desktop. Resynchronize after focus changes/reconnects and language switches without replacing physical Ctrl/Shift/AltGr handling or forcing unknown layouts to US.
- Reduce remote desktop rendering work when no views are attached, while preserving host applications and resumable sessions. Keep independent application views active until their own connection detaches.
- Render changed desktop content on demand, with prompt updates after idle input, bounded busy-frame pacing and low-rate callbacks for unviewed applications.
- Keep wheel-button delivery ordered with its exact pointer position, avoiding a cross-channel race when the compositor processes input quickly. Preserve held keyboard modifiers and fractional wheel accumulation.
- Negotiate exact tile reuse for moved or repeated screen regions. Resolve copies from one immutable committed baseline, preserve colours and atomic updates, and retain the original lossless path for older clients and agents.

These performance changes take effect in newly started host sessions after rebuilding. Existing retained sessions and their applications are left running with their current agent.

- Avoid a timing error between retained-session recovery and the final agent status update.

- Keep SSH terminal keyboard focus after right-click copy/paste, including when returning from another window, and return pasted input to the live prompt from scrollback.

- Recover retained desktops whose broker has already been reaped but left failed discovery files behind. Require a private exit record and a closed listener, preserve the old session directory for diagnosis, and leave live endpoints and unrelated processes untouched.
- Raise an explicitly activated independent application even when it already has keyboard focus but is covered by another host window. Check the real pointer target before sending clicks or wheel events, and preserve owned menu grabs and held modifiers.
- Record bounded, rate-limited host request errors in the application log while keeping routine focus warnings out of the GUI.
- Repair Windows' saved Restore placement as well as the visible window bounds after monitor changes. Keep maximized and minimized states intact, and make Maximize followed by Restore return to a reachable, usable window.
- Recover collapsed, zero-size or title-bar-only windows using their last usable normal size, with a bounded default if no usable size is available. Preserve valid small application windows, intentional monitor spans and active dragging.
- Keep normal placement in the correct Windows workspace coordinates, including top/left taskbars and monitors with negative origins. Never use minimized or maximized rectangles as normal sizes when replaying a deferred DPI change.
- Apply the shared recovery to the main window and captioned Files, editor and independent application windows. Recovery changes window geometry only; it does not restart connections or applications.

[Earlier release notes](docs/release-history.md).
