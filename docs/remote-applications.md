# Remote Applications

Remote Applications is a terminal-first connection for Linux graphical applications. The application runs and renders on the host; AetherDesk displays its individual windows on Windows using lossless image updates. It does not use the Windows X server and does not stream a full desktop alongside the application windows.

## Connect and launch

1. Create a connection and select **Remote Applications**. Enter the same SSH host, account and authentication information you would use for SSH. A saved private WireGuard configuration remains scoped to that connection.
2. If the application needs environment setup, enable **Run startup commands** and enter its exports or working directory. These commands run only when the private graphical session is first created. Reconnecting reuses the session's saved environment.
3. Connect. The tab contains an SSH terminal and a bounded application panel.
4. Enter a command in the application launcher, then press Enter or its launch icon. The launcher inherits the session's exports and working directory. Commands are explicitly user supplied and run with that account's existing permissions; they are not elevated. Application errors go to the host session log.
5. New top-level application windows open independently. Use the searchable list to reopen a closed view or bring an existing view forward. Up to eight independent views can be open at once. Additional applications remain running and can be opened after closing another view.

The terminal also points at this private graphical display. It supports the same terminal settings, folder synchronization, command history and keyboard behaviour as SSH connections. For applications that must survive disconnection, use the application launcher: foreground terminal jobs retain normal SSH shell lifetime semantics.

## Windows and controls

- Window captions follow the source application's title, including later document/layout name changes. Private native-window bindings keep identically named applications separate for focus, popup ownership and monitor placement.
- Drag each independent view between monitors. Content follows the monitor's Windows display scaling, with a separate 50–300% application zoom control. A move preserves logical window/text size and does not resize the host application. Manual resizing waits for native dragging and DPI changes to settle. Original capture pixels and colours remain lossless; client scaling uses nearest-neighbour sampling.
- Menus and associated popups use independent capture and stay inside the owner monitor's usable area, including menus the host opened above the application. Their input coordinates continue to address the original host menu. Transient focus changes recover silently and release held input; genuine connection and capture errors remain visible. Applications retain their original display, colours and open work.
- Keyboard and mouse input use the existing modifier, AltGr, keypad, wheel and focus-release handling. Queued input is sent in bounded, ordered batches, without adding a batching delay. Switching between independent views releases the old view's held modifiers first.
- Clipboard synchronization is scoped to the active application connection. The shared clipboard worker stays off the interface thread.
- Files opens the same separate SFTP window with transfer progress, resume, validation and editor support. Monitoring and host logs use the same authenticated connection.
- The application panel can be collapsed with its toolbar icon. Disconnect and End session stay on the right of the connection toolbar.

## Disconnect, reconnect and end

**Close a view** stops viewing that window; it does not close the host application. **Disconnect** closes the client connection and leaves the private host session and launcher applications running. Reconnect the same saved profile to return to those applications.

**End session** stops the private graphical session and its launcher process group. Save application work first. It does not end another user's session or an unrelated SSH, X11 or remote desktop connection. Remote Applications and Remote Desktop use separate retained identities, even if a saved profile is edited from one type to the other. Duplicate tabs retain their own recovery identities.

When a window being viewed independently is minimized inside its original remote
desktop, the agent temporarily withdraws that client from the host window manager
and keeps it mapped in a private capture holder. The independent window continues
painting and accepting real keyboard/mouse input; the holder does not cover or
intercept clicks on the remote desktop. Its original desktop taskbar entry is
absent during this period. Closing the independent view or disconnecting returns
the live application to the host window manager. The X11 server save-set protects
the application if its viewing helper disappears. This does not close/reopen the
application or transfer its design files to another display server. Real EDA
toolkits and transient-dialog families still require acceptance testing.

## Host requirements and scope

The completed AetherDesk package supplies the existing Linux agent runtime, Xwayland, XFWM and D-Bus tools. Session display sockets and authority files stay private to the host account. No additional inbound graphical port, root service or client-side X server is required.

This mode currently displays **X11 application windows rendered on the host**. GTK and Qt applications are directed to their X11 backends. It is not a promise of support for every Wayland-only application. The existing dimensions, packet, popup, queue and image memory bounds continue to apply.

The full private root still presents buffers and sends normal repaint callbacks. Full desktop pixel snapshots, video encoding and desktop image transmission are suppressed in this mode. Individual windows retain lossless tiles and damage tracking. Actual performance depends on application painting, host resources and the connection; it must be measured with the intended workload.

Updated helpers negotiate an event-driven application stream. Input is written independently of image reception, and the host keeps reading ordered input while its bounded image writer waits for a slow connection. Only one streamed image transaction is in flight; intermediate damage accumulates for the next complete update rather than building an image backlog. Active views have a 16 ms update spacing, background views 125 ms, and idle views primarily wait for X11 damage/window events. A slow periodic refresh catches missed notifications. Heartbeats and write deadlines retain disconnect cleanup.

The host reads damaged 256-pixel tiles instead of reading the whole application for a small repaint. Large repaint regions use one full read when that is cheaper. Windows retains immutable unchanged tiles and uploads changed regions into the existing texture, including all changes accumulated across skipped UI paints. An explicit screenshot reconstructs the complete image on demand. Menus retain ownership-checked capture, with cached window metadata invalidated by mapping, geometry and property events. All image compression stays lossless; colours, text and application rendering are unchanged. Older helpers fall back to the previous request/reply protocol on a fresh channel.

SSH+X11 remains available for compatibility. Its optimized forwarding keeps immediate small replies, lossless compression, private authentication and separate terminal/graphics transports, but cannot remove an application's synchronous X11 network exchanges. See [X.Org's performance discussion](https://www.x.org/Development/Documentation/Performance/) for that architectural limitation. The separation between seamless applications and full desktop sessions is also described in [Xpra's documentation](https://xpra-org.github.io/xpra/Usage/Seamless.html); AetherDesk reuses its own existing agent and independent-window implementation.

## Validation before publishing

Run the build kit's Windows and Linux regressions and its bundled X11 checks. Then use the rebuilt complete package to verify launching real applications, menus/dialogs, modifier shortcuts, AltGr/keypad, wheel, clipboard, simultaneous views, mixed-DPI monitor movement, SFTP/editor transfers, disconnect/reconnect, and explicit End session. Test Cadence using disposable work or read-only windows; automated fixtures do not establish compatibility with every EDA release or justify changing design libraries.
