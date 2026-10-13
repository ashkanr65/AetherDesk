# Remote agent deployment

AetherDesk deploys the Linux agent under the authenticated user's home directory at
`~/.aetherdesk/agent/<version>/`. It does not require root access on the remote host.

## Reproducible bundle build

The Windows package bundles an agent built on AlmaLinux 8 (glibc 2.28), matching the Rocky/Alma/RHEL 8 compatibility baseline. The build container uses RPM Fusion Free for the software H.264 encoder:

```sh
docker build -f agent/linux/Dockerfile.bundle --output type=local,dest=dist .
```

The output contains `ad-agent`, a private `libs/` directory, and
`runtime-manifest.json`. The packaging step includes the selected GStreamer plugins,
their dependency closure, the GStreamer plugin scanner, XWayland, and non-glibc
shared libraries. It intentionally does not bundle glibc or the ELF loader.

At deployment time AetherDesk verifies SHA-256 for every manifest entry before an
atomic rename. The launcher sets private `PATH`, `LD_LIBRARY_PATH`,
`GST_PLUGIN_PATH`, `GST_PLUGIN_SYSTEM_PATH_1_0`, and `GST_PLUGIN_SCANNER` values only
for the agent process. It does not modify the user's shell profile or system paths.

## Compatibility validation

For every supported distribution, deploy the same bundle and run:

```sh
~/.aetherdesk/agent/<version>/ad-agent --version
LD_LIBRARY_PATH="$HOME/.aetherdesk/agent/<version>/libs" \
  ldd "$HOME/.aetherdesk/agent/<version>/ad-agent"
```

Every non-glibc dependency must resolve either from the private `libs/` directory or
from the base operating system. A failure is a packaging defect; installing packages
on the remote host is not an accepted workaround.

The bundle must be tested on Ubuntu 20.04/22.04, Debian 11/12, Fedora 38, and Arch in
CI or dedicated clean virtual machines before it is promoted to a production release.
The repository's normal Linux job proves the bundle layout and checksum manifest,
but one Ubuntu runner is not evidence of the full distribution matrix.

## Troubleshooting

- A checksum mismatch causes deployment to fail before execution. Rebuild and
  republish the complete bundle; do not edit its files in place.
- A missing encoder means the required GStreamer plugin was not included by the
  packaging host. Confirm it is visible to `gst-inspect-1.0` while building.
- XWayland applications require the bundled `Xwayland` executable. Native Wayland
  applications continue to work if XWayland cannot start.
- Sessions are retained after transport loss. An explicit terminate request removes
  session state and temporary files.

## Windows client test

Download the complete Windows portable artifact from the Windows workflow and
extract every file together. The package contains `ad_ffi.dll`, the native Flutter
texture plugin, the GStreamer runtime, and `agent/linux-x86_64/`.

Create a Remote Desktop session for the Linux host and sign in through the app.
The host needs the selected desktop environment and `dbus-run-session`; the
agent supplies its private compositor/media runtime. XFCE is the initial test
target. Other desktop environments need separate live-session validation.

Verify the first frame, typing and modifier keys, mouse buttons and scrolling,
both directions of text clipboard transfer, resizing, and disconnect cleanup.
Ctrl+Alt+Escape releases captured keys. Clipboard sharing can be paused in the
desktop toolbar. SSH and SFTP regression tests run independently of the desktop.

Closing a Remote Desktop tab or AetherDesk detaches the view. Open the same saved
profile with the same host, account and desktop environment to resume it, even
after restarting the Windows app. Renaming the profile keeps its desktop identity.
Use **Session options > End remote session** to explicitly stop host applications.
The host must remain running; this is session retention, not recovery after a host
reboot or an application crash. Profiles from earlier builds start one new desktop
on their first connection with stable identity; old random-ID sessions are not
automatically adopted.

The **Full screen** button fills the current monitor. **Use all monitors** opens
one borderless view per connected Windows monitor, preserving their arrangement.
Move the pointer onto the overlay menu, or press Ctrl+Alt+Esc, to return to the tab.
The menu starts at the top of the screen but can be dragged anywhere by its
hamburger handle; when it is brought within the edge margin it docks horizontally
to the top or bottom, or vertically to the left or right side of the screen.
Monitor changes no longer end the presentation: monitors are re-detected
automatically (debounced) when they are added, removed, resized, rescaled or
rearranged, the views rebuild in place and the host receives exactly one fresh
monitor layout. A single-monitor presentation whose monitor disappears follows
the nearest remaining monitor instead of returning to the tab. Moving the main
window to another monitor also retargets a single-monitor presentation. Returning
to the tab sends the latest tab dimensions to the host so its window manager can
keep applications within the visible desktop. Supported layouts have at most
16 monitors, a 16384-pixel maximum canvas dimension and a 33554432-pixel total
canvas budget; larger physical arrangements are scaled down uniformly and an
impossible arrangement keeps the last good layout. If the Windows hardware decoder
advertises a smaller resolution limit, this client switches to software decoding
for the wider canvas. Re-detection is covered by the native tests; a final check
of Mixed-DPI and live monitor hotplug on the target Windows hardware is still
recommended.

The host keymap follows the active Windows input layout, independently of the
Windows display language. Spanish (Spain), Latin American Spanish, US and US
International are included; unknown custom layouts are left unchanged. Verify
ñ, accents, AltGr symbols, shortcuts and keypad keys after switching input layout,
both in the tab and fullscreen.

Audio and native file drag-and-drop are not part of this client path.
