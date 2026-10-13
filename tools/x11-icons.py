#!/usr/bin/env python3
"""Supply missing EWMH icons on this session's forwarded X display.

Uses system Xlib and GdkPixbuf; no Python packages or window manager required.
Existing application icons are always retained. Optional user overrides live
in ~/.config/aetherdesk/x11-icons.json and map WM_CLASS to image filenames.
"""
import configparser
import ctypes as C
import ctypes.util
import json
import os
from pathlib import Path
import re
import shutil
import select


def icon_roots():
    data = os.environ.get("XDG_DATA_DIRS", "/usr/local/share:/usr/share").split(":")
    return [Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))),
            *[Path(p) for p in data if p]]


def desktop_icons(roots):
    result = {}
    for root in roots:
        for path in (root / "applications").glob("*.desktop"):
            try:
                parser = configparser.ConfigParser(interpolation=None, strict=False)
                parser.read(path, encoding="utf-8")
                entry = parser["Desktop Entry"]
                icon = entry.get("Icon")
                if not icon:
                    continue
                keys = [path.stem, entry.get("StartupWMClass", ""), entry.get("Name", "")]
                for key in keys:
                    if key:
                        result.setdefault(key.lower(), icon)
            except (OSError, ValueError, KeyError, configparser.Error):
                continue
    return result


def cadence_program(names):
    for name in names:
        if re.search(r"(^|[^a-z])(innovus|encounter)(64)?($|[^a-z])", name.lower()):
            return "innovus"
        if re.search(r"(^|[^a-z])(virtuoso|icfb)(64)?($|[^a-z])", name.lower()):
            return "virtuoso"
    return None


def find_icon(names, entries, roots, overrides, executable=None):
    for name in names:
        override = overrides.get(name.lower())
        if override:
            path = Path(override).expanduser()
            if path.is_file():
                return path
    product = cadence_program(names)
    names = list(names) + ([product] if product else [])
    candidates = list(dict.fromkeys([entries.get(n.lower(), n.lower()) for n in names]))
    for name in candidates:
        direct = Path(name)
        if direct.is_absolute() and direct.is_file():
            return direct
        if "/" in name or name in ("", ".", ".."):
            continue
        for root in roots:
            for suffix in ("", ".png", ".svg", ".xpm", ".ico"):
                for relative in (f"icons/hicolor/48x48/apps/{name}{suffix}",
                                 f"icons/hicolor/64x64/apps/{name}{suffix}",
                                 f"icons/hicolor/scalable/apps/{name}{suffix}",
                                 f"icons/Adwaita/48x48/legacy/{name}{suffix}",
                                 f"pixmaps/{name}{suffix}"):
                    path = root / relative
                    if path.is_file():
                        return path
    # Cadence installations often expose product artwork outside XDG themes.
    if product:
        program_path = executable or shutil.which(product) or (shutil.which("encounter") if product == "innovus" else shutil.which("icfb"))
        bases = [Path(os.environ[key]) for key in ("CDS_INST_DIR", "CDSROOT", "INNOVUS_HOME", "CDS_HOME") if os.environ.get(key)]
        if program_path:
            bases.extend(list(Path(program_path).resolve().parents)[:5])
        for base in bases:
            for directory in ("share/icons", "share/pixmaps", "tools/dfII/etc/icons", "tools/innovus/etc/icons",
                              "tools.lnx86/innovus/etc/icons", "tools/innovus/share/icons", "tools/fe/gift/icons", "icons"):
                for stem in (product, product.capitalize(), "encounter" if product == "innovus" else "icfb", "cadence", "Cadence"):
                    for suffix in (".png", ".xpm", ".svg", ".ico"):
                        path = base / directory / (stem + suffix)
                        if path.is_file():
                            return path
    return None


def clock_icon():
    # A recognizable fallback for the stock Xclock client, which has no color icon.
    size = 48
    pixels = []
    for y in range(size):
        for x in range(size):
            radius = (x - 23.5) ** 2 + (y - 23.5) ** 2
            color = 0
            if radius < 22 ** 2:
                color = 0xfff6f7fa
            if 19 ** 2 < radius < 22 ** 2:
                color = 0xff25364a
            if (22 <= x <= 24 and 10 <= y <= 25) or (23 <= x <= 34 and 23 <= y <= 25):
                color = 0xff25364a
            pixels.append(color)
    return [size, size, *pixels]


class WindowEvent(C.Structure):
    _fields_ = [("type", C.c_int), ("serial", C.c_ulong), ("send_event", C.c_int),
                ("display", C.c_void_p), ("event", C.c_ulong), ("window", C.c_ulong)]


class PropertyEvent(C.Structure):
    _fields_ = [("type", C.c_int), ("serial", C.c_ulong), ("send_event", C.c_int),
                ("display", C.c_void_p), ("window", C.c_ulong), ("atom", C.c_ulong)]


class XEvent(C.Union):
    _fields_ = [("type", C.c_int), ("window", WindowEvent),
                ("property", PropertyEvent), ("padding", C.c_long * 24)]


class XIcons:
    def __init__(self):
        self.x = C.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
        self._bind("XOpenDisplay", C.c_void_p, C.c_char_p)
        self._bind("XDefaultRootWindow", C.c_ulong, C.c_void_p)
        self._bind("XInternAtom", C.c_ulong, C.c_void_p, C.c_char_p, C.c_int)
        self._bind("XGetWindowProperty", C.c_int, C.c_void_p, C.c_ulong, C.c_ulong,
                   C.c_long, C.c_long, C.c_int, C.c_ulong, C.POINTER(C.c_ulong),
                   C.POINTER(C.c_int), C.POINTER(C.c_ulong), C.POINTER(C.c_ulong),
                   C.POINTER(C.c_void_p))
        self._bind("XQueryTree", C.c_int, C.c_void_p, C.c_ulong, C.POINTER(C.c_ulong),
                   C.POINTER(C.c_ulong), C.POINTER(C.POINTER(C.c_ulong)), C.POINTER(C.c_uint))
        self._bind("XChangeProperty", C.c_int, C.c_void_p, C.c_ulong, C.c_ulong, C.c_ulong,
                   C.c_int, C.c_int, C.c_void_p, C.c_int)
        self._bind("XFree", C.c_int, C.c_void_p)
        self._bind("XFlush", C.c_int, C.c_void_p)
        self._bind("XCloseDisplay", C.c_int, C.c_void_p)
        self._bind("XSelectInput", C.c_int, C.c_void_p, C.c_ulong, C.c_long)
        self._bind("XPending", C.c_int, C.c_void_p)
        self._bind("XNextEvent", C.c_int, C.c_void_p, C.POINTER(XEvent))
        self._bind("XConnectionNumber", C.c_int, C.c_void_p)
        self.error_handler = C.CFUNCTYPE(C.c_int, C.c_void_p, C.c_void_p)(lambda *_: 0)
        self.x.XSetErrorHandler(self.error_handler)
        self.display = self.x.XOpenDisplay(None)
        if not self.display:
            raise RuntimeError("The forwarded X display is unavailable")
        self.root = self.x.XDefaultRootWindow(self.display)
        self.net_icon = self.atom("_NET_WM_ICON")
        self.cardinal = self.atom("CARDINAL")
        self.wm_class = self.atom("WM_CLASS")
        self.wm_hints = self.atom("WM_HINTS")
        self.wm_name = self.atom("WM_NAME")
        self.net_name = self.atom("_NET_WM_NAME")
        self.net_pid = self.atom("_NET_WM_PID")
        self.x.XSelectInput(self.display, self.root, 1 << 19)  # SubstructureNotifyMask

    def watch(self, window):
        self.x.XSelectInput(self.display, window, (1 << 22) | (1 << 17))

    def changed_windows(self):
        # Sleep on the X socket, not a timer. No round trips for idle windows.
        if not self.x.XPending(self.display):
            select.select([self.x.XConnectionNumber(self.display)], [], [])
        dirty = set()
        event = XEvent()
        for _ in range(256):
            if not self.x.XPending(self.display):
                break
            self.x.XNextEvent(self.display, C.byref(event))
            if event.type in (16, 19, 21):  # Create, map, reparent
                window = event.window.window
                self.watch(window)
                dirty.add(window)
            # EDA titles can change on every pointer move. Titles are a fallback
            # at map/class change, not a reason for repeated network round trips.
            elif event.type == 28 and event.property.atom in (self.wm_class, self.wm_hints, self.net_icon, self.net_pid):
                dirty.add(event.property.window)
            elif event.type == 17:  # Destroy
                dirty.discard(event.window.window)
        return dirty

    def _bind(self, name, result, *arguments):
        function = getattr(self.x, name)
        function.restype, function.argtypes = result, list(arguments)

    def atom(self, name):
        return self.x.XInternAtom(self.display, name.encode(), 0)

    def property(self, window, atom, limit=1024):
        kind, count, remaining = C.c_ulong(), C.c_ulong(), C.c_ulong()
        fmt, data = C.c_int(), C.c_void_p()
        status = self.x.XGetWindowProperty(self.display, window, atom, 0, limit, 0, 0,
            C.byref(kind), C.byref(fmt), C.byref(count), C.byref(remaining), C.byref(data))
        try:
            if status or not data.value:
                return None
            if fmt.value == 8:
                return C.string_at(data, count.value)
            if fmt.value == 32:
                return list(C.cast(data, C.POINTER(C.c_ulong))[:count.value])
            return None
        finally:
            if data.value:
                self.x.XFree(data)

    def windows(self):
        clients = self.property(self.root, self.atom("_NET_CLIENT_LIST"), 4096)
        if isinstance(clients, list) and clients:
            return clients
        windows = self.children(self.root)
        # Reparenting WMs put application windows one level below frame windows.
        for window in list(windows):
            if len(windows) >= 4096:
                break
            windows.extend(self.children(window))
        return windows[:4096]

    def children(self, window):
        root, parent, children, count = C.c_ulong(), C.c_ulong(), C.POINTER(C.c_ulong)(), C.c_uint()
        if not self.x.XQueryTree(self.display, window, C.byref(root), C.byref(parent), C.byref(children), C.byref(count)):
            return []
        try:
            return list(children[:min(count.value, 4096)])
        finally:
            if children:
                self.x.XFree(children)

    def set_icon(self, window, pixels):
        data = (C.c_ulong * len(pixels))(*pixels)
        self.x.XChangeProperty(self.display, window, self.net_icon, self.cardinal, 32, 0, data, len(pixels))
        self.x.XFlush(self.display)


def load_pixels(path):
    pixbuf = C.CDLL(ctypes.util.find_library("gdk_pixbuf-2.0") or "libgdk_pixbuf-2.0.so.0")
    objects = C.CDLL(ctypes.util.find_library("gobject-2.0") or "libgobject-2.0.so.0")
    glib = C.CDLL(ctypes.util.find_library("glib-2.0") or "libglib-2.0.so.0")
    create = pixbuf.gdk_pixbuf_new_from_file_at_scale
    create.argtypes, create.restype = [C.c_char_p, C.c_int, C.c_int, C.c_int, C.POINTER(C.c_void_p)], C.c_void_p
    objects.g_object_unref.argtypes = [C.c_void_p]
    glib.g_error_free.argtypes = [C.c_void_p]
    error = C.c_void_p()
    image = create(os.fsencode(path), 48, 48, 1, C.byref(error))
    if error.value:
        glib.g_error_free(error)
    if not image:
        return None
    try:
        values = []
        for name in ("width", "height", "rowstride", "n_channels"):
            getter = getattr(pixbuf, "gdk_pixbuf_get_" + name)
            getter.argtypes, getter.restype = [C.c_void_p], C.c_int
            values.append(getter(image))
        width, height, stride, channels = values
        getter = pixbuf.gdk_pixbuf_get_pixels
        getter.argtypes, getter.restype = [C.c_void_p], C.POINTER(C.c_ubyte)
        data = getter(image)
        if not 0 < width <= 48 or not 0 < height <= 48 or channels not in (3, 4):
            return None
        pixels = [width, height]
        for y in range(height):
            for x in range(width):
                i = y * stride + x * channels
                alpha = data[i + 3] if channels == 4 else 255
                pixels.append((alpha << 24) | (data[i] << 16) | (data[i + 1] << 8) | data[i + 2])
        return pixels
    finally:
        objects.g_object_unref(image)


def watch_parent_process():
    """Stop the SSH helper with its owning shell (not a transient subshell)."""
    import threading
    import time

    value = os.environ.pop("AETHERDESK_X11_PARENT", "")
    if not value.isdecimal() or int(value) <= 1:
        return
    parent = int(value)

    def identity():
        try:
            # Start time disambiguates PID reuse; comm may contain spaces.
            with open(f"/proc/{parent}/stat") as source:
                return source.read().rsplit(") ", 1)[1].split()[19]
        except (OSError, IndexError):
            return None

    initial = identity()
    if initial is None:
        os._exit(0)

    def watch():
        while True:
            time.sleep(2)
            if identity() != initial:
                os._exit(0)

    threading.Thread(target=watch, daemon=True).start()


def main():
    watch_parent_process()
    x = XIcons()
    roots = icon_roots()
    entries = desktop_icons(roots)
    try:
        config = Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "aetherdesk/x11-icons.json"
        overrides = {k.lower(): v for k, v in json.loads(config.read_text()).items() if isinstance(v, str)}
    except (OSError, ValueError, AttributeError):
        overrides = {}
    cache = {}
    try:
        windows = x.windows()
        for window in windows:
            x.watch(window)
        while True:
            for window in windows:
                if x.property(window, x.net_icon, 2):
                    continue
                hints = x.property(window, x.wm_hints, 10)
                value = x.property(window, x.wm_class)
                if not isinstance(value, bytes):
                    continue
                names = tuple(n for n in value.decode("utf-8", "replace").split("\0") if n)
                title = x.property(window, x.net_name, 256) or x.property(window, x.wm_name, 256)
                product = cadence_program(names) or cadence_program([title.decode("utf-8", "replace") if isinstance(title, bytes) else ""])
                if product:
                    names += (product,)
                # Cadence can provide a generic legacy pixmap. Replace that only
                # when genuine installed artwork (or an explicit override) exists.
                if hints and hints[0] & 4 and not product and not any(n.lower() in overrides for n in names):
                    continue
                executable = None
                if product:
                    pid = x.property(window, x.net_pid, 1)
                    if isinstance(pid, list) and pid and 0 < pid[0] < 2 ** 31:
                        try:
                            executable = os.readlink("/proc/{}/exe".format(pid[0]))
                        except OSError:
                            pass
                key = (names, executable)
                if key not in cache:
                    path = find_icon(names, entries, roots, overrides, executable)
                    try:
                        pixels = load_pixels(path) if path else None
                    except (OSError, ValueError):
                        pixels = None
                    if pixels is None and "xclock" in [n.lower() for n in names]:
                        pixels = clock_icon()
                    # Bound per-class misses for applications with changing titles.
                    if len(cache) >= 256:
                        cache.clear()
                    cache[key] = pixels
                if cache[key]:
                    x.set_icon(window, cache[key])
            windows = x.changed_windows()
    finally:
        x.x.XCloseDisplay(x.display)


if __name__ == "__main__":
    main()
