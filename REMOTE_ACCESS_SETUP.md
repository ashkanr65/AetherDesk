# AetherDesk Graphical Compatibility Setup

> **Current X11 packaging:** the Windows package includes its private X11
> runtime. No separate VcXsrv installation is needed. The RDP instructions
> below describe the older compatibility path; see
> [desktop-and-vpn.md](docs/desktop-and-vpn.md) for current embedded-desktop behavior.

AetherDesk now provides two real graphical connection modes. They use the same verified SSH connection as the terminal and SFTP workspace; the application does not simulate a successful graphical session.

## SSH + X11

AetherDesk includes VcXsrv, its libraries, fonts and keyboard data in the `x11`
folder beside `aetherdesk.exe`. Install AetherDesk or extract its complete
portable ZIP; there is no separate Windows X-server setup. See
[bundled-x11.md](docs/bundled-x11.md) for packaging and validation details.

On the Linux host, OpenSSH must allow X11 forwarding and `xauth` must be installed:

```text
X11Forwarding yes
```

After changing the SSH server configuration, restart `sshd`. Create an **SSH + X11** profile in AetherDesk, connect, and run an X11 application such as `xclock`, `gedit`, `nautilus`, or the X11 version of the engineering application you need.

AetherDesk launches VcXsrv with access control enabled, a new Xauthority file, clipboard integration, multi-window mode, and OpenGL acceleration. The remote machine receives a one-time cookie rather than the local VcXsrv cookie. Do not add `-ac` to the launch options.

Wayland-native applications must support XWayland or be launched with their X11 backend. A pure Wayland window cannot be carried by the X11 protocol.

## Full Linux Remote Desktop

The Linux server must have:

- `xrdp` and `xorgxrdp` installed;
- the selected desktop environment installed;
- the `xrdp` and `xrdp-sesman` services running; and
- alternate-shell selection enabled in `/etc/xrdp/sesman.ini`:

```text
[Security]
AllowAlternateShell=true
```

Restart xrdp after changing that file. Typical systemd commands are:

```bash
sudo systemctl enable --now xrdp
sudo systemctl restart xrdp xrdp-sesman
```

Install the desktop command corresponding to the profile selection:

| Selection | Command AetherDesk verifies |
|---|---|
| XFCE | `startxfce4` |
| KDE Plasma | `startplasma-x11` |
| GNOME | `gnome-session` |
| MATE | `mate-session` |
| Cinnamon | `cinnamon-session` |
| LXQt | `startlxqt` |

Do not expose TCP 3389 to the network for AetherDesk. The client binds a unique address inside `127.0.0.0/8` and forwards it through the already verified SSH session to `127.0.0.1:3389` on Linux.

The generated RDP session enables clipboard sharing, audio playback and capture, dynamic resolution, multiple monitors, compression, bandwidth detection, smart-card/WebAuthn redirection, and automatic reconnection. SFTP remains the preferred file-transfer path because it has explicit transfer progress, concurrency, retry, and resume. Server-side xrdp audio modules are required for Linux audio redirection.

For password-authenticated SSH profiles, the RDP handoff uses a session-scoped Windows credential and removes it after launch. It is not written to the `.rdp` file. SSH-key profiles may still receive the standard xrdp password prompt because the Linux RDP server performs its own login.

## Troubleshooting

- **Connection failed rather than spinning forever:** AetherDesk bounds TCP, authentication, shell, and remote-command setup. Read the visible error and the local audit log.
- **Built-in X11 files missing or incomplete:** reinstall AetherDesk or extract the entire portable ZIP. Do not copy only its executable or install a separate X server.
- **X11 application says it cannot open display:** verify `X11Forwarding yes`, install `xauth`, reconnect, and check that the application is using X11/XWayland.
- **Remote Desktop detects NX/X2Go:** the Linux service is healthy, but this build is missing AetherDesk's embedded NX-compatible renderer. Do not install xrdp merely to hide that client-side implementation gap.
- **Remote Desktop reports that no service is available:** install `xrdp` plus `xorgxrdp` only if the administrator intentionally chooses the RDP compatibility path.
- **Desktop selection disabled:** set `AllowAlternateShell=true` under `[Security]` in `sesman.ini`, then restart both xrdp services.
- **Audio is absent:** install the xrdp audio integration appropriate for the Linux distribution and confirm the `rdpsnd` channel is enabled server-side.
