# Remote Desktop architecture, gap analysis, and implementation decision

Date: 2026-08-27

## Executive decision

AetherDesk will use a dedicated **Aether Stream** subsystem as its primary
Linux desktop mode: a signed, unprivileged Linux agent deployed through the
verified SSH/SFTP connection, paired with an in-process Windows decoder and
GPU texture renderer. It will use damage-aware lossless tiles for text and
static UI, hardware video codecs for moving regions, and independent control,
input, clipboard, audio, file, and display channels.

Two compatibility engines remain separate:

- an NX compatibility engine is the shortest path to the reported existing
  X2Go/XFCE server, but it is not the strategic renderer;
- an embedded RDP engine is appropriate for hosts intentionally running xrdp,
  but xrdp cannot consume an NX/X2Go session.

The current build does **not** contain either the NX renderer or Aether Stream.
It must not claim otherwise. It contains an xrdp compatibility launcher that
opens Windows Remote Desktop through a loopback-only SSH tunnel.

## Root cause

The failing and working products take different protocol paths:

```text
Working X2Go
  Windows X2Go client -> SSH -> x2gostartagent -> x2goagent -> NX stream

Current AetherDesk RDP compatibility
  AetherDesk -> verified SSH -> loopback tunnel -> xrdp:3389 -> mstsc.exe
```

RDP and NX are not wire-compatible. Selecting XFCE only selects a desktop
launcher. It does not translate NX into RDP. The server is healthy; AetherDesk
authenticates and detects the server, then stops because it has no client-side
NX decoder/rendering endpoint.

## Complete component inventory

| Area | Present | Partial | Missing |
|---|---|---|---|
| SSH transport | Password/key auth, host verification, exec, PTY, local forwarding | No multiplexed stream priorities | QUIC transport |
| SFTP | Browser, upload/download, concurrent jobs, resume | Drag/drop behavior depends on Flutter desktop support | Desktop-session drive channel |
| Session start | xrdp/NX/desktop probe; XFCE/KDE/GNOME/MATE/Cinnamon/LXQt command mapping | xrdp alternate-shell launch | X2Go start/resume/terminate orchestration; agent supervisor |
| NX | Detects `x2gostartagent` and `x2goagent` | None beyond detection | nxproxy integration, client X endpoint, cache/codec state, input, clipboard, audio, resume, rendering |
| RDP | SSH tunnel, `.rdp` profile, temporary Windows credential, external `mstsc.exe` | Host must provide xrdp+xorgxrdp | Embedded FreeRDP/IronRDP renderer and channels |
| Display | Flutter UI and GPU-composited widgets | `ad-display` scaffold | Desktop frame model, textures, cursor, color conversion, monitor layout |
| Stream client | Crate exists | Generated scaffold only | Framing, bounds checks, flow control, tile cache, codec negotiation, telemetry |
| Linux agent | Architecture described | No build/deployment code | Capture, encode, input, clipboard, audio, session lifecycle, signed artifacts |
| X11 | SSH `x11-req`, cookie isolation, packet validation | Local display uses VcXsrv | Bundled/sandboxed endpoint or Aether agent X11 adapter |
| Native bridge | Real SSH/SFTP ABI | Capability JSON | Remote desktop ABI and Flutter texture integration |

The Rust crates `ad-display`, `ad-stream-client`, `ad-rdp-bridge`, `ad-vnc`,
and `ad-x11-bridge` are scaffolds. `ad_rdp_connect()` and `ad_vnc_connect()`
return explicit not-implemented errors.

## Options evaluated

### A — bundle the official X2Go client stack

This has the lowest server compatibility risk and supports the current host.
It is not a small library integration. The official Windows build combines a
Qt client, nxproxy, a full VcXsrv display server, optional PulseAudio, SSH
helpers, session scripts, and separate windows. The most recent official
Windows release documented by the project is from 2020 and includes legacy
components. Shipping that binary unchanged would create unacceptable security,
maintenance, signing, and GPL/source-distribution risk. It also would not meet
the in-tab renderer requirement.

**Decision:** do not silently download or rebrand the old official binary.

### B — embed/rebuild NX-compatible components

Rebuilding current `nx-libs` supplies nxproxy, but nxproxy is not a renderer by
itself. Windows still needs a complete secured X endpoint, X2Go command/session
orchestration, input/clipboard/audio integration, packaging, and licensing
work. This is viable as a deliberately maintained compatibility engine and is
the shortest route to the current server, but not the lowest total product
risk.

**Decision:** maintain as an optional compatibility milestone, gated by a
current-source reproducible build, SBOM, license review, signing, sandboxing,
and end-to-end tests. Never depend on an installed X2Go or VcXsrv application.

### C — Aether Stream agent and protocol

This is the only option that meets the standalone, embedded, adaptive,
GPU-aware, multi-monitor, and long-term Wayland/XWayland requirements. It has
the highest initial engineering cost but avoids inheriting an aging NX/X-server
client architecture and allows measurable latency/bandwidth optimization.

**Decision:** selected primary architecture.

## Updated module diagram

```text
Windows AetherDesk
  Flutter session/tab controller
    +-- SSH terminal workspace
    +-- SFTP workspace
    +-- SSH+X11 compatibility
    +-- RemoteDesktopCoordinator
          +-- AetherStreamClient
          |     framing / flow control / telemetry
          |     tile cache + hardware video decoder
          |     D3D texture / cursor / monitor compositor
          +-- NxCompatibilityEngine (optional milestone)
          +-- EmbeddedRdpEngine (optional compatibility)
                 |
          verified SSH transport
                 |
Linux user account
  ~/.aetherdesk/agent/<version>/aetherdesk-agent
    +-- signed session supervisor and reconnect tokens
    +-- X11 adapter: XDamage + capture for XFCE/legacy sessions
    +-- Wayland adapter: virtual outputs + PipeWire/DMA-BUF
    +-- XWayland application compatibility
    +-- adaptive tile/video encoder
    +-- input, clipboard, audio, files, monitors, audit
```

## Performance strategy

1. Keep input/control on a high-priority bounded channel so frames and file
   transfers cannot block interaction.
2. Send lossless content-addressed tiles for terminals, text, schematics,
   icons, and low-change regions.
3. Switch moving regions to H.264/HEVC/AV1 only when negotiated hardware
   encode/decode and measured encode delay make it beneficial.
4. Track damage and avoid full-frame work; keep the cursor local.
5. Use adaptive congestion control, frame pacing, and late-frame dropping.
6. Preserve the remote session independently of the transport and resume with
   cache hashes rather than a full cold frame.
7. Treat monitors as independent virtual outputs and streams.
8. Measure p50/p95 input-to-photon latency, frame time, bandwidth, CPU/GPU,
   memory, startup, resize, and resume against the same X2Go/xrdp baselines.

No “faster than every product” claim is accepted without controlled benchmark
evidence at LAN, 25 ms, 60 ms, and 120 ms RTT profiles.

## Security and production gates

- upload only signed, hash-pinned agent artifacts after SSH host verification;
- run as the authenticated user without root and expose no public listener;
- bind an ephemeral protocol nonce and version to the SSH session;
- bound every message, allocation, frame dimension, clipboard, and file path;
- sandbox the agent with available Linux controls and the codec workers on
  Windows;
- fuzz protocol/decoder boundaries and test disconnect, corruption, and replay;
- produce an SBOM and complete GPL/LGPL/codec patent review;
- redact credentials, clipboard, keystrokes, and frame contents from logs.

## Delivery plan and honest status

1. **Completed foundation:** accurate capability reporting, protocol probe,
   audit diagnostics, immutable multi-session tabs, five-second notification
   lifecycle, desktop choices including MATE/Cinnamon, and a truthful welcome
   page.
2. **Protocol milestone:** freeze framed messages, limits, capability
   negotiation, reconnect identity, and fuzz corpus.
3. **Agent MVP:** XFCE/X11 full desktop, input, cursor, clipboard, dynamic
   resize, lossless tiles, signed SSH deployment.
4. **Codec/performance:** moving-region hardware video, audio, adaptive network
   control, multi-monitor, WAN benchmark gates.
5. **Wayland/XWayland:** KDE/GNOME/Cinnamon modern sessions plus XWayland; keep
   X11 adapter for XFCE/MATE and legacy EDA applications.
6. **Compatibility engines:** embedded RDP and, only if its maintenance gates
   are met, rebuilt NX compatibility.

A production implementation of the missing agent, protocol, codecs, renderer,
and platform integration is a multi-person engineering project. This document
is deliberately not used as proof that those modules already exist.

## Primary references

- X2Go server flow: <https://wiki.x2go.org/doku.php/wiki:advanced:x2goserver-in-detail>
- X2Go Windows build components: <https://wiki.x2go.org/doku.php/wiki:development:build-howto-mswin:x2goclient>
- X2Go Windows 4.1.2.2 release: <https://wiki.x2go.org/doku.php/doc:release-notes-mswin:x2goclient-4.1.2.2>
- Arctica nx-libs: <https://github.com/ArcticaProject/nx-libs>
- xrdp and xorgxrdp: <https://github.com/neutrinolabs/xrdp>
- FreeRDP: <https://github.com/FreeRDP/FreeRDP>
- Wayland architecture: <https://wayland.freedesktop.org/architecture.html>
- Xfce Wayland roadmap: <https://wiki.xfce.org/releng/wayland_roadmap>
- PipeWire: <https://docs.pipewire.org/>
