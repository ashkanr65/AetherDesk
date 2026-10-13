# Forwarded application icons

SSH+X11 sessions now start a small icon helper on the forwarded display. It
leaves existing `_NET_WM_ICON` color icons alone. For applications
without an icon it resolves `StartupWMClass` and `Icon` from installed desktop
entries, XDG icon directories, and common Cadence installation directories.
The stock xclock client also has a built-in clock fallback.
Updates are event-driven: idle windows are not repeatedly queried over SSH.
Cadence legacy pixmaps can be replaced only when installed artwork is found.
Innovus/Encounter aliases, product window titles, installation environment
variables and the window's process executable help locate that artwork.

The helper needs Python 3.6 or newer and the host's libX11. Loading PNG, SVG, XPM and other
application artwork additionally needs the system GdkPixbuf library. These
are optional: an unavailable helper never prevents the terminal or X11 apps
from opening. No additional window manager is launched; VcXsrv multiwindow
mode already provides one. No authentication or X11 access control is disabled.

Cadence distributions differ in their artwork paths and window classes. If
a site does not install a desktop entry, put a mapping in the **remote** file
`~/.config/aetherdesk/x11-icons.json`, for example:

```json
{
  "Virtuoso": "/path/to/installed/cadence/virtuoso.png",
  "Innovus": "/path/to/installed/cadence/innovus.xpm"
}
```

Use the actual class reported by `xprop WM_CLASS` and actual installed artwork.
Reconnect the SSH+X11 tab after changing the mapping. This mapping supplies
missing color icons only; it can replace legacy `WM_HINTS` pixmaps but never an
existing `_NET_WM_ICON` color icon.
Proprietary Cadence logos are not redistributed with AetherDesk.
