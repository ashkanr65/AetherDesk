# AetherDesk 0.1.2 build 7 - Remote mouse input

- Correct right-click and middle/wheel-click mappings in both native Windows capture and the GUI input fallback.
- Normalize horizontal scrolling between the two client input paths so changing focus/capture routes does not reverse its direction.
- Translate vertical wheel direction at the Wayland boundary, keeping smooth and discrete scrolling consistent for native Wayland and X11 applications.
- Correct Back/Forward mouse button IDs, retain compatibility with earlier side-button IDs and ignore unsupported buttons without stopping desktop capture.
- Add required regression checks for client-route parity, wire mappings, Wayland axis frames and actual X11 button/wheel delivery.

Validation: the completed build passed all seven stages. Required Windows and
Linux regression checks passed, including client input-route parity, protocol
mappings, Wayland axis direction and real X11 button/wheel delivery. Packaged
mouse/EDA zoom acceptance across tab, fullscreen and all-monitor modes remains a
separate manual check. Reconnect with the updated client and agent to receive
the complete fix.
