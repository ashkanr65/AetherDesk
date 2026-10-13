# AetherDesk Remote Display Architecture Evaluation

Date: 2026-08-27

## Executive decision

AetherDesk should **not** replace X11 with Wayland as though Wayland were a
network protocol. Wayland is local compositor/client IPC. It can provide a much
better capture and composition boundary for a purpose-built remote agent, but a
separate streaming protocol, encoder, input path, session manager, and client
renderer are still required.

The recommended product architecture is hybrid:

1. **Near-term XFCE compatibility:** embed and sign an NX-compatible client
   engine so the existing X2Go/NX service can be used without installing an
   external Windows client. This is the only path compatible with the reported
   server without changing its display service. It requires license review and
   a real Windows renderer; `nxproxy` alone is not a complete client.
2. **Strategic AetherDesk mode:** deploy a signed, unprivileged AetherDesk Linux
   agent over the verified SSH/SFTP channel. Use a Wayland-first compositor and
   capture path for supported desktops, with XWayland and an X11-session adapter
   for legacy applications and current XFCE deployments.
3. **RDP compatibility:** embed FreeRDP or IronRDP for hosts that intentionally
   provide xrdp. Do not launch `mstsc.exe` in the final standalone product.
4. **Do not make Wayland-only the default for XFCE yet.** Xfce 4.20 calls its
   Wayland support experimental, and the Xfce roadmap says feature parity is not
   yet the 4.22 target.

This is an architecture decision, not a claim that a missing display engine has
already been implemented. The current repository contains placeholders for the
display, stream, X11, and RDP engines. The NX screenshot will remain a product
blocker until one of the two real client/agent implementations above ships.

## What the current failure proves

The successful X2Go connection and failed AetherDesk connection use different
wire protocols:

```text
Working path
Windows X2Go client -> SSH -> x2gostartagent -> x2goagent -> NX stream

Current AetherDesk compatibility path
AetherDesk -> SSH -> loopback TCP forward -> xrdp:3389 -> mstsc.exe
```

Selecting XFCE only selects a session launcher. It does not convert NX into RDP.
The remote probe correctly finds `x2gostartagent` and `x2goagent`; AetherDesk
then stops because it has no NX decoder/renderer. The following repository
facts are conclusive:

- `core/rust/crates/ad-display`, `ad-stream-client`, `ad-x11-bridge`, and
  `ad-rdp-bridge` contain generated stubs;
- `ad_rdp_connect()` always returns null with “RDP engine is not implemented”;
- the Flutter RDP path starts the Windows `mstsc.exe` process; and
- the Flutter X11 path searches for VcXsrv, despite the standalone requirement.

No XFCE command, timeout, readiness-marker, or server package change can repair
that client-side protocol gap.

## Protocol comparison

Ratings are relative engineering assessments for AetherDesk's per-user Linux
desktop use case, not vendor benchmark results. Actual performance must be
measured under the benchmark matrix below.

| Architecture | Interactive latency | WAN bandwidth | Compression/caching | CPU/GPU path | Multi-monitor/session resume | Fit for AetherDesk |
|---|---|---|---|---|---|---|
| SSH X11 forwarding | Good on LAN, often poor as RTT rises | Often high/chattery | SSH compression only; no full-desktop video adaptation | App may render remotely; local X server composites | Weak resume; per-window placement varies | Compatibility fallback only |
| Wayland protocol alone | Excellent local IPC; not a WAN transport | Not applicable | None for networks | Strong DMA-BUF, Vulkan/EGL, and damage model | Compositor-dependent | Agent substrate, not transport |
| XWayland | Small local compatibility cost | Not a transport | None by itself | Lets X11 apps feed a Wayland compositor | Depends on compositor/agent | Required legacy bridge |
| NX/X2Go | Strong for interactive X11 over constrained WAN | Low to moderate | Protocol-aware compression, caching, coalescing | Mature X11 path; aging architecture | Mature suspend/resume | Best immediate match for the reported XFCE server |
| RDP + xrdp | Usually strong on LAN/WAN; workload-dependent | Moderate; modern GFX helps | Bitmap, progressive, H.264-family paths depending on peers | Client decode can be accelerated; Linux server acceleration varies | Strong channels, resize, clipboard, resume | Valuable compatibility backend; requires xrdp server |
| FreeRDP | Client implementation of RDP, not a server choice | Follows negotiated RDP features | Supports multiple RDP codecs | Embeddable C library; platform backends | Broad RDP channel support | Replace external `mstsc.exe` for standalone RDP |
| SPICE | Strong in its VM-oriented design center | Moderate to low | QUIC/LZ/GLZ and video heuristics | Designed around QEMU/guest integration | Clipboard/audio/multi-monitor via guest agent | Poor fit for arbitrary SSH Linux hosts |
| RustDesk-style frame streaming | Strong when capture/codec/route are well selected | Adaptive video codecs | VP8/VP9/AV1 and H.264/H.265 paths | Hardware encode/decode where available | Product-specific | Useful design reference, not a per-user desktop-session backend |
| AnyDesk | Vendor reports strong latency/bandwidth | Proprietary | Proprietary DeskRT | Implementation details are not public | Product-specific | Competitive benchmark only; not a reproducible architecture source |

### X11

X11 was designed as a network-transparent request/event protocol. That is
valuable for seamless remote applications, but many applications create
round-trip-sensitive request patterns. SSH protects the channel but does not
turn those exchanges into an adaptive remote-desktop stream. NX improves X11
WAN behavior by inserting protocol-aware compression and caching.

Strengths:

- current XFCE compatibility;
- mature application behavior;
- natural seamless-window semantics; and
- NX/X2Go session persistence.

Limits:

- RTT sensitivity for raw forwarding;
- an X server is still needed on Windows;
- weak modern GPU-buffer integration compared with Wayland; and
- a large legacy protocol/security surface.

### Wayland

Wayland clients submit buffers and damage to a compositor. Modern clients can
share DMA-BUF GPU buffers with Vulkan/EGL synchronization. This makes a
compositor an effective place to observe damage, combine surfaces, manage
virtual outputs, and feed a hardware encoder without unnecessary copies.

Wayland does **not** define WAN compression, congestion control, reconnect,
authentication, audio transport, or a Windows decoder. Its normal wire path is
a local Unix socket. Therefore “Wayland is faster than X11 remotely” is not a
valid comparison until a remoting implementation is specified.

### XWayland

XWayland runs an X server as a Wayland client. It is essential for legacy
applications such as older EDA/CAD tools while the remote session is composed
through a Wayland pipeline. It is a compatibility layer, not a network
transport. Full legacy XFCE sessions require a dedicated X11-session adapter;
XWayland alone does not magically turn `xfwm4` into a Wayland compositor.

### NX/X2Go

X2Go's server documentation describes `x2gostartagent` allocating a display and
ports, starting `x2goagent`, and using NXv3-derived libraries for graphical
compression and caching. That exactly matches the service detected on the
reported host. It is the lowest-deployment-risk path for that host, but a
production Windows client needs much more than a command invocation:

- NX session negotiation and resume;
- an embedded, secured display endpoint/renderer;
- input, clipboard, audio, filesystem, and monitor channels;
- signed packaging and sandboxing; and
- GPL/component license compliance.

### RDP/xrdp/FreeRDP

xrdp supports reconnect, dynamic resizing, two-way clipboard, drive channels,
and optional audio/microphone redirection. Current xorgxrdp releases have RDP
Graphics Pipeline support and report frame-rate/bandwidth improvements over the
older path, although hardware-accelerated encoding remains configuration- and
version-dependent. It may outperform NX for video-like workloads or fast LANs;
NX may remain better for low-change X11 UI on high-RTT links. Only controlled
benchmarks can decide.

FreeRDP is an Apache-licensed embeddable RDP client library with GFX and codec
support. It is the preferred route for a standalone RDP compatibility engine.
It does not make an NX-only server speak RDP.

### SPICE, RustDesk, and AnyDesk

SPICE has useful multi-monitor, clipboard, audio, image compression, and video
streaming concepts, but its strongest integration is with QEMU and a guest
agent. Requiring that stack would be a poor match for arbitrary servers reached
over SSH.

RustDesk demonstrates a pragmatic adaptive frame-stream architecture with
software VP8/VP9/AV1 and hardware H.264/H.265 choices, plus direct/relay routing.
It is a useful codec and connection-selection reference. Its support-oriented
screen capture model does not itself create independent XFCE/KDE/GNOME sessions.

AnyDesk publishes performance claims for its proprietary DeskRT codec, but the
codec and transport internals are not public enough to reproduce or compare
architecturally. Treat its numbers as a benchmark target only.

## Proposed standalone architecture

```text
Windows AetherDesk
  Session/tab controller
  Native D3D texture + hardware decoder
  Input / clipboard / audio / monitor channels
  Adaptive stream receiver and tile cache
                |
      verified SSH connection
      independent prioritized channels
                |
Linux ~/.aetherdesk/agent/<signed-version>/
  authenticated session supervisor
  +-- Wayland capture adapter (modern sessions)
  |     compositor damage + DMA-BUF/PipeWire + virtual outputs
  +-- X11 capture adapter (XFCE/legacy full desktops)
  |     XDamage + shared-memory/GPU capture
  +-- XWayland compatibility (legacy apps in Wayland sessions)
  +-- adaptive tile/video encoder
  +-- PipeWire audio
  +-- clipboard and session persistence service
```

### Deployment and trust

1. Authenticate SSH and verify the host fingerprint before uploading anything.
2. Select an agent artifact by architecture and libc compatibility.
3. Verify an embedded Ed25519 release signature and SHA-256 digest locally and
   remotely.
4. Upload through SFTP to a versioned user-owned directory; never require root.
5. Launch over an SSH exec channel with a one-time nonce and protocol version.
6. Bind the application handshake to the verified SSH session; open no public
   listening port.
7. Apply message-size, frame-dimension, clipboard, file, and codec limits before
   allocation; fuzz every decoder and protocol parser.
8. Use seccomp/Landlock where available, redact all secrets, and audit lifecycle
   events without keystrokes or clipboard contents.

### Display and codec policy

- Use lossless or visually lossless tiles for text, terminals, schematics, and
  low-change desktop regions.
- Switch moving regions to AV1 or H.264 based on negotiated hardware support,
  encode delay, loss, and available bandwidth.
- Cache glyphs, icons, cursor shapes, and stable tiles by content hash.
- Keep the cursor local and acknowledge input separately from frame delivery.
- Maintain separate prioritized channels for input/control, audio, clipboard,
  and display so bulk frames cannot block input.
- Represent monitors as independent virtual outputs and streams; permit a
  combined surface only as a compatibility fallback.
- Preserve sessions under a per-user supervisor so closing a client transport
  suspends rather than destroys the desktop.

## Benchmark assumptions and acceptance gates

No performance superiority claim should ship without tests against the same
machine, resolution, application, and network profile.

### Network profiles

| Profile | RTT | Available bandwidth | Loss/jitter |
|---|---:|---:|---:|
| LAN | 1 ms | 1 Gbit/s | 0% / <1 ms |
| Office VPN | 25 ms | 100 Mbit/s | 0.1% / 3 ms |
| Branch WAN | 60 ms | 20 Mbit/s | 0.5% / 8 ms |
| Difficult WAN | 120 ms | 8 Mbit/s | 1% / 15 ms |

### Workloads

- terminal scrolling and text editing;
- file manager and settings UI;
- browser page scrolling;
- 1080p video region;
- large EDA/CAD schematic pan/zoom;
- 1080p, 4K, and dual-monitor layouts; and
- disconnect/resume with a running application.

### Measurements

- p50/p95 input-to-photon latency from high-speed camera or timestamped probe;
- delivered frames per second and p95 frame interval;
- median/p95 bandwidth and retransmission volume;
- remote encode and local decode CPU/GPU time;
- working set per session;
- cold start, warm reconnect, and monitor-resize time; and
- visual quality using SSIM/VMAF plus lossless-text inspection.

Initial gates should be expressed against measured X2Go and xrdp baselines, for
example: no worse p95 interaction latency for terminal/file-manager workloads,
lower or equal median bandwidth at equal text quality, and resume without losing
session state. Absolute “beats every product” claims are not testable without
the same third-party configuration and workload.

## Migration roadmap and effort

### Phase 0 — capability truth and test harness (completed/in progress)

- keep NX-only detection distinct from server failure;
- stop representing placeholder crates as implemented engines;
- build reproducible network emulation and input-to-photon instrumentation; and
- preserve independent session tabs and lifecycle logs.

### Phase 1 — embedded NX interoperability for the reported host

Estimated effort: 4–8 engineer-months plus packaging and legal review.

- evaluate `nx-libs`/`nxproxy` and a complete Windows rendering endpoint;
- implement session start/list/resume/terminate parsing;
- integrate input, clipboard, audio, monitor, and lifecycle channels;
- isolate and fuzz the legacy protocol boundary; and
- ship all runtime components inside the signed installer.

Exit gate: the supplied XFCE/X2Go server starts and resumes a desktop without
X2Go Client, MobaXterm, VcXsrv, or another installed application.

### Phase 2 — embedded RDP compatibility

Estimated effort: 3–6 engineer-months.

- replace `mstsc.exe` with FreeRDP/IronRDP behind the Rust FFI boundary;
- render frames into a native Flutter texture;
- implement clipboard, audio, monitor, smart-card/WebAuthn, and reconnect tests;
- retain the verified SSH loopback tunnel for xrdp hosts.

### Phase 3 — AetherDesk agent alpha

Estimated effort: 12–20 engineer-months.

- signed unprivileged deployment and protocol handshake;
- persistent session supervisor;
- X11 damage capture first for XFCE parity;
- lossless tiles plus one hardware/software video codec;
- input, clipboard, audio, resize, and reconnect.

### Phase 4 — Wayland-first production engine

Estimated cumulative effort: 60–100 engineer-months, typically 12–18 months for
a focused 5–7 person team.

- headless/nested compositor adapters and DMA-BUF/PipeWire capture;
- XWayland and desktop-specific adapters;
- adaptive tile/video region classification;
- multi-monitor, accessibility, IME, and color/HDR work;
- protocol fuzzing, sandboxing, enterprise policy, fleet upgrades; and
- sustained WAN benchmark tuning against X2Go, xrdp, RustDesk, and AnyDesk.

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| NX/X server licensing | Distribution may require source/notice obligations | Complete legal review before choosing components; keep process boundary replaceable |
| Wayland desktop fragmentation | Different compositors expose different capture/session APIs | Versioned adapters and explicit capability negotiation |
| XFCE Wayland immaturity | Current server may lose features or fail to start | Keep X11 adapter; do not force Wayland |
| GPU/codec diversity | Hardware paths differ by vendor/distro | Runtime negotiation, software fallback, conformance matrix |
| SSH TCP head-of-line blocking | Display bursts can delay input/audio | Separate prioritized SSH channels; optional authenticated direct transport later |
| Agent supply chain | Uploaded executable is a privileged target within the user account | Signed artifacts, pinned hashes, rollback, sandbox, transparent audit |
| Protocol/parser vulnerabilities | Remote peers supply hostile dimensions and media | Rust boundaries, strict limits, fuzzing, memory/time budgets |

## Primary sources

- Wayland architecture and damage/compositor model:
  <https://wayland.freedesktop.org/architecture.html>
- Wayland protocol and local Unix-socket model:
  <https://wayland.freedesktop.org/docs/book/Protocol.html>
- Wayland GPU buffer exchange:
  <https://wayland.freedesktop.org/docs/book/Architecture.html>
- XWayland architecture:
  <https://wayland.freedesktop.org/docs/book/Xwayland.html>
- Weston headless, RDP, PipeWire, GL, and Vulkan backends:
  <https://wayland.pages.freedesktop.org/weston/toc/running-weston.html>
- Xfce Wayland roadmap:
  <https://wiki.xfce.org/releng/wayland_roadmap>
- Xfce 4.20 Wayland warning:
  <https://xfce.org/about/tour>
- X2Go server/NX session lifecycle:
  <https://wiki.x2go.org/doku.php/wiki:advanced:x2goserver-in-detail>
- Arctica NX libraries:
  <https://github.com/ArcticaProject/nx-libs>
- xrdp capabilities:
  <https://github.com/neutrinolabs/xrdp>
- FreeRDP library:
  <https://github.com/FreeRDP/FreeRDP>
- SPICE features:
  <https://www.spice-space.org/features.html>
- PipeWire low-latency media framework:
  <https://docs.pipewire.org/>
- RustDesk published codec architecture:
  <https://rustdesk.com/docs/en/>
- AnyDesk's public performance/DeskRT claims:
  <https://www.anydesk.com/en/performance>
