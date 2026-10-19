> **Rust conversion in progress:** this is the isolated AetherDesk-Rust clone.
> The native implementation is under ui/rust. It is not yet feature-equivalent.
> tools/rust-build-kit packages the native development preview; tools/build-kit
> still packages the reference Flutter application. See RUST-FEATURE-PARITY.md
> for the detailed migration status. The documentation below describes the
> original application's behavior, not completed native Rust parity.

# AetherDesk — Software Architecture, TRD, and PRD

**Working product name:** AetherDesk  
**Document type:** Architecture + Technical Requirements Document + Product Requirements Document  
**Primary platform:** Windows 10/11 x64  
**Secondary future platforms:** Linux client, macOS client  
**Target:** MobaXterm-style productivity suite with X2Go-class or better remote Linux GUI performance  
**Date:** 2026-08-10  

---

## Current implementation status

The Windows client currently implements independent reorderable session tabs; password and SSH private-key authentication (including Ed25519 keys and encrypted-key passphrases); a real interactive PTY terminal; explicit first-use host-fingerprint approval with later key-change rejection; and an integrated local/remote SFTP explorer. SFTP transfers support drag and drop, concurrent jobs, visible transfer state, retry, and offset-based resume.

Credential policies include **Remember Username**, **Remember Password**, **Ask Every Time**, and **Never Save Credentials**. Passwords and key passphrases are never written to session profiles. On Windows, saved secrets are stored through Windows Credential Manager, with an additional DPAPI layer when the user context permits it; optional master-password protection derives DPAPI entropy with PBKDF2-HMAC-SHA256. Connection, host-key, credential, and transfer events are written to a rotating local audit log without secrets.

**SSH + X11** is implemented through an authenticated, isolated VcXsrv display bundled inside the Windows package; users do not install an external X server. AetherDesk makes the SSH `x11-req`, uses a one-time remote cookie, validates every forwarded X11 setup packet, and substitutes the real local cookie only on Windows. It never disables X-server access control. See [bundled X11](docs/bundled-x11.md).

**Aether Stream Remote Desktop** now has a versioned ARPP transport, persistent Linux agent, headless Smithay compositor with XWayland, encoded display pipeline, input forwarding, resumable session lifecycle, file staging bridge, and bidirectional Wayland/XWayland clipboard transport. The deployment bootstrap can install a checksum-verified private runtime next to the agent, without requiring root access or system GStreamer packages. See [the deployment runbook](docs/runbooks/remote-agent-deployment.md).

The Windows client now connects this pipeline to a registered Flutter pixel-buffer texture, H.264 decoding, physical keyboard and mouse input, resizing, and bidirectional text clipboard sharing. The Windows workflow packages the decoder runtime and matching Linux agent. Remote Desktop is enabled only when those packaged components are available. Frames are copied into Flutter's native texture; this is not the planned zero-copy D3D11 path. Live host compatibility and performance still require verification.

Saved Remote Desktop profiles reattach to the same running host desktop after closing a tab or AetherDesk. The explicit **Disconnect**/**End remote session** actions stop its applications so the next connection starts a new session. Windows desktop controls include fullscreen, one view per connected monitor with live hotplug re-detection, and active input-layout synchronization (including Spanish). Ctrl+Alt+Esc or the hover menu returns to the tab; the menu can be dragged by its hamburger handle and snaps to any screen edge.

**AI Assistant** opens a LangGraph chat in the left sidebar while keeping the
active terminal or remote desktop visible. Connect NVIDIA, OpenRouter (free and
paid models), Groq, or an OpenAI-compatible API, then choose a model and a Windows or
connected SSH workspace. Model and provider lists support search and alphabetical
sorting. Enter sends; Ctrl+Enter adds a line. Provider keys use the Windows
credential vault; edits and commands use action, message or chat approvals.
The key menu can remove saved APIs, and the history menu can delete individual
chats or clear history. Drag the sidebar edge to resize it. The assistant can
open workspace files, control connection tabs and inspect attached connection
screens with a vision model. It can also create reusable Windows or SSH scripts
in **Tools**, with installation and execution covered by the chosen approval scope. Both installer and portable packages include Python, LangGraph and all
agent dependencies; end users install no runtime. Model access still follows the
provider account's terms and limits. See [AI Assistant setup, saved tools and
package checks](docs/bundled-assistant.md).

**Application logging** writes a persistent, rotated `logs\app.log` (JSONL) covering the Rust core, FFI errors, Dart/UI errors and notices, native presentation events, X11 launch failures, assistant service output, and the fetched host-side `agent.log`. The **Logs** sidebar panel shows Application, Host, Audit and Problems tabs with level, source and search filtering, follow-tail, export and an open-folder action; the Audit tab keeps its security-only behavior. Secrets are redacted before anything is written. See [logging](docs/logging.md).

RDP/VNC compatibility, remote audio playback, and native Wayland file drag-and-drop remain incomplete. Existing SSH/SFTP transfers are available separately. The architecture/PRD below describes the intended product and must not be interpreted as proof that every listed subsystem is production-ready.

---

## 0. Executive Summary

AetherDesk is designed as a lightweight, modular, cross-platform remote-workspace client combining:

- Multi-tab SSH terminal
- SFTP file browser
- SSH tunneling and SOCKS proxying
- RDP and VNC clients
- X11 forwarding compatibility
- Remote Linux application publishing
- Session and credential management
- Plugin architecture
- High-performance remote GUI transport optimized for WAN links

The recommended architecture is a **Rust core + native GPU rendering + Flutter desktop UI** model, with a **pluggable remote-display subsystem**. Instead of relying only on classic X11 forwarding, AetherDesk should implement a hybrid strategy:

1. **SSH terminal/SFTP/tunneling** as first-class citizens.
2. **RDP/VNC compatibility** through mature libraries.
3. **X11 forwarding compatibility mode** via a managed local X server bridge.
4. **High-performance Linux application publishing** using a custom lightweight Linux streaming agent based on Wayland/headless compositor concepts, with an NX-inspired caching/compression protocol.

The key performance insight is that X2Go outperforms classic X11 forwarding because it avoids shipping raw X11 protocol chatter over WAN links. It proxies, compresses, caches, coalesces, and encodes graphical output closer to a remote-desktop protocol than a remote-X protocol.

AetherDesk should therefore not try to win by improving raw X11 forwarding alone. It should treat X11 as compatibility mode and build its primary high-performance path around a modern, adaptive, region-aware, cache-friendly streaming protocol.

---

# Part I — Architecture Document

---

## 1. Why X2Go Is Faster Than MobaXterm for Remote Linux GUI

The comparison depends heavily on workload and configuration, but in typical WAN usage, X2Go often feels faster than MobaXterm when MobaXterm is used primarily through X11 forwarding. The main reasons are protocol-level, not merely branding.

### 1.1 X11 Forwarding Is Chatty and Round-Trip Sensitive

Classic X11 forwarding sends the X protocol over SSH. X11 was designed for local networks and assumes relatively low-latency access to a display server.

Problems over WAN:

- Many small request/response messages
- Frequent round trips for atoms, properties, fonts, extensions, grabs, and events
- Poor coalescing of drawing operations
- High sensitivity to RTT
- Many operations are synchronous or semi-synchronous
- Heavy CPU overhead from protocol parsing and context switching
- Poor adaptation to bandwidth constraints

A single application action can generate many X11 messages. On a 100–200 ms WAN link, this becomes noticeable.

### 1.2 X2Go Uses NX-Style Compression and Caching

X2Go is based on NX technology. The core idea is not to forward raw X11 traffic naively, but to place an agent on the remote Linux side that understands the graphical workload.

NX-style optimizations include:

- X protocol interception/proxying
- Display output caching
- Pixmap and glyph caching
- Compression of images and drawing operations
- Differential screen updates
- Region coalescing
- Lossy JPEG encoding for complex graphics
- Lossless encoding for text-critical regions when needed
- Session persistence and reconnection
- Reduced round trips compared with raw X11 forwarding

This makes X2Go far more WAN-friendly than classic X11 forwarding.

### 1.3 X2Go Has Session Persistence

X2Go sessions can survive disconnections. The user reconnects and resumes the same desktop or application session.

This improves perceived performance and usability because:

- Applications do not need to restart
- Window state is preserved
- Remote session setup cost is amortized
- Users avoid repeated environment initialization

MobaXterm can support persistent terminal sessions via SSH multiplexing or tmux/screen, but GUI X11-forwarded applications are generally less robust across disconnections.

### 1.4 MobaXterm Is a Broad Tool, Not a Dedicated NX-Style Streaming Client

MobaXterm is excellent as an integrated toolbox: X server, SSH terminal, SFTP, RDP, VNC, X forwarding, macros, and more. But its remote Linux GUI path often depends on X11 forwarding and a local X server.

That model has advantages:

- Broad compatibility
- Easy integration with existing SSH servers
- No special remote agent required

But disadvantages over WAN:

- More protocol round trips
- Less efficient graphical compression
- Less adaptive bitrate control
- Less efficient window persistence
- Higher latency sensitivity

### 1.5 Practical Conclusion

X2Go is faster in many WAN scenarios because it behaves more like a purpose-built remote display protocol. MobaXterm’s X11 forwarding path behaves more like a compatibility layer over a protocol not designed for high-latency links.

Therefore, AetherDesk should:

- Support X11 forwarding for compatibility
- Not treat X11 forwarding as the primary high-performance GUI path
- Implement or integrate an NX-inspired or Wayland-based streaming agent for serious WAN performance

---

## 2. Protocol Comparison

### 2.1 Comparison Matrix

| Protocol / Approach | Model | WAN Performance | Latency Behavior | Linux App Publishing | Implementation Complexity | Compatibility | Recommendation |
|---|---:|---:|---:|---:|---:|---:|---|
| X11 forwarding over SSH | Raw X protocol tunneled through SSH | Poor to moderate | High RTT sensitivity | Good for legacy X apps | Low to medium | Very high | Compatibility mode only |
| NX / X2Go-style protocol | Remote X proxy with caching/compression | Good to very good | Much better than X11 | Good for X apps, limited Wayland | High | High on X11, lower on Wayland | Strong inspiration for custom mode |
| RDP | Full remote desktop / RemoteApp protocol | Good to excellent with modern codecs | Good, especially with UDP/GFX | Possible on Linux via xrdp, but not ideal | Medium using FreeRDP | Excellent for Windows, moderate for Linux | Use for compatibility and Windows targets |
| VNC / RFB | Framebuffer streaming | Moderate | Often poor on WAN without tuning | Limited | Low to medium | High | Compatibility fallback |
| Wayland remoting / PipeWire / portal | Modern Linux display capture/input | Potentially excellent | Depends on implementation | Strong future path | High, fragmented | Growing | Best long-term Linux app publishing path |
| Custom protocol | Purpose-built adaptive streaming | Potentially best | Best if designed correctly | Excellent if agent exists | Very high | Depends on agent deployment | Recommended for differentiator |

---

### 2.2 X11 Forwarding

**Strengths:**

- No remote agent needed beyond standard SSH server
- Works with many existing Linux systems
- Good compatibility with legacy X11 applications
- Simple user mental model: `ssh -X` or `ssh -Y`

**Weaknesses:**

- High latency over WAN
- Chatty protocol
- Poor bandwidth adaptation
- Weak modern security defaults unless carefully configured
- Does not naturally handle Wayland apps
- Poor persistence
- Requires local X server on Windows

**Use in AetherDesk:**

- Compatibility mode
- Managed local X server bridge
- Not primary performance path

---

### 2.3 NX / X2Go-Style Protocols

**Strengths:**

- Designed for WAN efficiency
- Caching and compression
- Better interactive responsiveness
- Session persistence
- Good for X11 applications

**Weaknesses:**

- Requires remote agent
- Historically tied to X11
- Licensing and compatibility concerns depending on implementation
- Less natural fit for Wayland-only environments
- More complex deployment than plain SSH X forwarding

**Use in AetherDesk:**

- Architectural inspiration
- Potential compatibility mode if legally and technically viable
- Do not depend on proprietary NX components
- Build clean-room adaptive protocol with similar principles

---

### 2.4 RDP

**Strengths:**

- Mature, high-performance remote desktop protocol
- Excellent Windows support
- Good codec support, including hardware acceleration
- RemoteApp model exists
- FreeRDP is a strong open-source client base
- Supports clipboard, drives, printers, audio, redirection

**Weaknesses:**

- Linux support depends on xrdp or similar server stacks
- Remote Linux application publishing is not as elegant as custom Wayland streaming
- Configuration can be complex
- Some advanced features are Windows-centric
- Licensing and patent considerations require review

**Use in AetherDesk:**

- First-class protocol for Windows endpoints
- Useful for Linux remote desktop via xrdp
- Not the sole solution for seamless Linux application publishing

---

### 2.5 VNC

**Strengths:**

- Simple and widely supported
- Good fallback
- Many server implementations
- Easy to understand

**Weaknesss:**

- Often poor WAN performance unless heavily tuned
- Limited semantic understanding of UI
- Weak modern security in many deployments
- Limited application publishing model
- Inconsistent encoding support across servers

**Use in AetherDesk:**

- Compatibility viewer
- Support common encodings: Tight, ZRLE, JPEG, cursor pseudo-encodings
- Not primary performance path

---

### 2.6 Wayland Remoting

**Strengths:**

- Modern Linux display stack
- Better security model than raw X11
- More control over surfaces, buffers, and input
- Better fit for GPU pipelines
- Potential for low-overhead capture and streaming
- Aligns with future Linux desktop direction

**Weaknesses:**

- No single universal remote protocol equivalent to X11 forwarding
- Fragmentation across compositors
- Requires agent or compositor integration
- Input injection and clipboard integration require care
- Headless and virtual display setups vary by distro

**Use in AetherDesk:**

- Preferred long-term path for Linux application publishing
- Implement an AetherDesk Linux agent that creates a controlled Wayland environment or integrates with existing compositors

---

### 2.7 Custom Protocol

**Strengths:**

- Full control over latency, compression, caching, and adaptation
- Can optimize for text/UI/video mixed workloads
- Can prioritize input and control messages
- Can implement seamless application windows
- Can support SSH-only fallback and optional UDP acceleration

**Weaknesses:**

- Highest engineering cost
- Requires robust fuzzing and security review
- Requires remote agent deployment
- Must handle many Linux environment edge cases

**Use in AetherDesk:**

- Core differentiator
- Implement as an adaptive remote presentation protocol
- Use SSH for secure bootstrap and fallback
- Optionally use direct secure UDP/QUIC path for media when permitted

---

## 3. Recommended Optimal Architecture

### 3.1 Core Recommendation

Use a **hybrid modular architecture**:

- **Rust core engine** for SSH, terminal emulation, SFTP, tunneling, protocol multiplexing, plugin runtime, and performance-critical services.
- **Flutter desktop UI** for modern cross-platform UI, with native interop for GPU surfaces and platform integration.
- **C/C++ integration layer** where mature libraries are required: FreeRDP, VNC, codecs, and possibly X server components.
- **Pluggable remote display engines**:
  - SSH terminal
  - SFTP
  - X11 bridge
  - RDP engine
  - VNC engine
  - AetherDesk Stream Engine, a custom adaptive protocol
- **Linux streaming agent** for high-performance remote Linux app publishing.
- **SSH-first secure transport** with optional direct secure media path using QUIC/DTLS where allowed.

---

### 3.2 Key Architecture Principles

1. **SSH is the trust anchor.**  
   All remote operations should be bootstrapable through SSH.

2. **X11 is compatibility, not the future.**  
   Support it, but do not design the performance strategy around it.

3. **The remote GUI path must be adaptive.**  
   Text, UI, images, and video require different encoding strategies.

4. **Input priority is sacred.**  
   Keyboard, mouse, clipboard, and window-control messages must not be blocked behind large frame updates.

5. **Use hardware acceleration wherever possible.**  
   Decode on client GPU, encode on remote GPU when available.

6. **Avoid heavyweight UI runtimes.**  
   No Electron for the main shell if low CPU/RAM and fast startup are mandatory.

7. **Design for partial failure.**  
   Networks drop, SSH channels stall, plugins crash, remote agents die. The client must recover gracefully.

8. **Plugin system must be sandboxed.**  
   Plugins should not be able to read credentials or capture sessions without explicit permission.

---

### 3.3 High-Level Architecture Diagram

```text
+----------------------------------------------------------------------------------+
|                               AetherDesk Client                                  |
|                                                                                  |
|  +------------------------+   +----------------------+   +--------------------+ |
|  | Flutter Desktop UI     |   | Command Palette      |   | Session Manager    | |
|  | - Tabs / Panes         |   | - Search             |   | - Profiles         | |
|  | - Terminal Views       |   | - Actions            |   | - Tags / Folders   | |
|  | - File Browser         |   |                      |   | - Layouts          | |
|  +----------+-------------+   +----------+-----------+   +---------+----------+ |
|             |                             |                          |           |
|             +--------------+--------------+--------------------------+           |
|                            |                                                     |
|                 +----------v-----------+                                         |
|                 | Core Broker / API    |                                         |
|                 | Rust FFI / IPC       |                                         |
|                 +----+-----+-----+-----+                                         |
|                      |     |     |                                               |
|        +-------------+     |     +----------------+                              |
|        |                   |                      |                              |
| +------v------+    +-------v--------+     +-------v--------+                    |
| | Terminal    |    | SSH Stack      |     | Credential     |                    |
| | Engine      |    | - Auth         |     | Vault          |                    |
| | - VT parser |    | - Channels     |     +----------------+                    |
| | - PTY       |    | - SFTP         |                                           |
| | - GPU text  |    | - Tunnels      |     +----------------+                    |
| +-------------+    | - Agent fwd    |     | Plugin Host    |                    |
|                    +-------+--------+     | WASM / Native  |                    |
|                            |              +----------------+                    |
|                            |                                                     |
|             +--------------+----------------+------------------+                 |
|             |              |                |                  |                 |
|      +------v-----+ +------v-----+  +-------v------+  +--------v-------+        |
|      | SFTP File  | | Tunnel     |  | Display      |  | X11 Bridge     |        |
|      | Browser    | | Manager    |  | Orchestrator |  | Managed X srv  |        |
|      +------------+ +------------+  +---+----+-----+  +----------------+        |
|                                        |    |                                   |
|                    +-------------------+    +------------------+                |
|                    |                                         |                 |
|             +------v-------+                          +------v-------+          |
|             | RDP Engine   |                          | VNC Engine   |          |
|             | FreeRDP      |                          | RFB client   |          |
|             +--------------+                          +--------------+          |
|                                                                                  |
|             +------------------------------------------------------+             |
|             | AetherDesk Stream Engine                             |             |
|             | - Adaptive encoder/decoder                           |             |
|             | - Tile cache / glyph cache / image cache             |             |
|             | - Input prioritization                               |             |
|             | - SSH channel transport                              |             |
|             | - Optional QUIC/DTLS media transport                 |             |
|             +------------------------------------------------------+             |
+----------------------------------------------------------------------------------+
                                     |
                                     | SSH / optional secure UDP media
                                     v
+----------------------------------------------------------------------------------+
|                             Remote Linux Host                                    |
|                                                                                  |
|  +----------------------+     +----------------------+     +------------------+ |
|  | sshd                 |     | AetherDesk Agent     |     | xrdp / VNC       | |
|  | - auth               |<--->| - session supervisor |     | compatibility    | |
|  | - channels           |     | - Wayland compositor |     +------------------+ |
|  +----------------------+     | - XWayland bridge    |                           |
|                               | - input injection    |     +------------------+ |
|                               | - clipboard sync     |     | X11 / Wayland    | |
|                               | - audio optional     |     | applications     | |
|                               | - encoder pipeline   |     +------------------+ |
|                               +----------------------+                           |
+----------------------------------------------------------------------------------+
```

---

## 4. Complete System Design

## 4.1 Client Components

### 4.1.1 Application Shell

Responsibilities:

- Main window management
- Tabs, panes, splitters
- Command palette
- Theme support
- Session tree
- Settings UI
- Plugin UI panels
- Notification center
- Global search

Design constraints:

- Must start quickly
- Must avoid blocking the UI thread
- Must render smoothly at 60 FPS where possible
- Must remain responsive during file transfers and remote connections

---

### 4.1.2 Core Broker

Responsibilities:

- Central message bus
- Session lifecycle orchestration
- API boundary between UI and engines
- Event routing
- Error normalization
- Telemetry aggregation
- Permission checks for plugins

Implementation style:

- Rust library exposed to UI via FFI or platform channels
- Async event loop
- Strongly typed internal messages
- UI never talks directly to sockets or SSH internals

---

### 4.1.3 Session Manager

Responsibilities:

- Store sessions, folders, tags, and templates
- Launch sessions
- Restore layouts
- Handle jump hosts and proxies
- Manage reconnect policies
- Track session health
- Support session groups

Session types:

- SSH terminal
- SFTP
- Local terminal
- RDP
- VNC
- X11 app
- AetherDesk remote app
- Tunnel-only session
- Composite workspace containing multiple panes

---

### 4.1.4 Credential Manager

Responsibilities:

- Store passwords, key passphrases, tokens, and certificates
- Integrate with Windows Credential Manager
- Optional encrypted local vault
- Support SSH agent integration
- Support hardware key unlock for app vault
- Prevent secrets from entering logs
- Provide scoped credential release to engines

Security requirements:

- Encrypt at rest
- Zeroize secrets when possible
- Use OS-provided protection where available
- Require user confirmation for plugin credential access
- Support per-session credential scoping

---

### 4.1.5 SSH Stack

Responsibilities:

- SSH client protocol
- Host key verification
- Known hosts management
- Password, public key, keyboard-interactive, certificate auth
- SSH agent forwarding with user consent
- Channel multiplexing
- PTY sessions
- SFTP subsystem
- Local/remote/dynamic port forwarding
- SSH compression negotiation
- Rekeying and keepalive
- Jump host support

Recommended implementation:

- Rust SSH stack where feasible
- Optional integration with OpenSSH for maximum compatibility
- Abstract transport layer so implementation can be swapped

---

### 4.1.6 Terminal Engine

Responsibilities:

- VT parser and terminal state machine
- ANSI/DEC/xterm compatibility
- True color, italic, underline, strikethrough
- Hyperlinks
- Mouse reporting
- Bracketed paste
- Focus events
- Scrollback buffer
- Search
- Selection and clipboard
- Unicode and wide-character handling
- GPU-accelerated glyph rendering
- Local PTY support via ConPTY on Windows
- Remote PTY over SSH

Performance targets:

- Low keystroke-to-display latency
- Efficient scrolling
- Minimal CPU during idle
- Efficient handling of high-throughput output

---

### 4.1.7 SFTP File Browser

Responsibilities:

- Remote file browsing
- Local file browsing
- Drag-and-drop transfer
- Transfer queue
- Resume support
- Parallel transfers
- Permission editing
- Symlink handling
- File hashing
- Quick preview
- Remote file editing handoff to external editor
- Bookmarking

Design:

- Async I/O
- Read-ahead directory listing
- Thumbnail generation off main thread
- Transfer progress observability

---

### 4.1.8 Tunnel Manager

Responsibilities:

- Local port forwarding
- Remote port forwarding
- Dynamic SOCKS5 proxy
- Automatic port allocation
- Tunnel health checks
- Reconnect on failure
- Per-session tunnel rules
- Tunnel usage metrics
- Optional tunnel sharing across sessions

---

### 4.1.9 Display Orchestrator

Responsibilities:

- Choose display engine based on session type
- Manage display surface lifecycle
- Coordinate input routing
- Coordinate clipboard
- Coordinate resize and DPI changes
- Provide common frame presentation interface
- Monitor latency and bandwidth

Engines managed:

- RDP
- VNC
- X11 bridge
- AetherDesk Stream Engine
- Future WebRTC or SPICE plugins

---

### 4.1.10 RDP Engine

Responsibilities:

- Connect to RDP servers
- Support NLA
- Support RDP Gateway where possible
- Support RemoteApp mode where possible
- Support clipboard
- Support drive redirection as optional feature
- Hardware-accelerated decoding
- Multi-monitor support later

Implementation:

- FreeRDP integration through C ABI wrapper
- Render into shared texture or GPU surface

---

### 4.1.11 VNC Engine

Responsibilities:

- RFB client
- Common auth types
- Tight, ZRLE, raw, cursor encodings
- JPEG support where appropriate
- Clipboard support
- Scaling and remote resize where supported

---

### 4.1.12 X11 Bridge

Responsibilities:

- Provide local X server integration on Windows
- Manage `DISPLAY` and `XAUTHORITY`
- Handle SSH X11 channel forwarding
- Support trusted and untrusted forwarding modes
- Launch/manage local X server process
- Coordinate window focus and clipboard where possible

Implementation options:

1. Manage an external open-source X server process.
2. Integrate a separate X server component as an optional module.
3. Use WSLg-based compatibility path where appropriate.

Recommendation for MVP:

- Do not build a full X server from scratch.
- Manage an external/local X server process or use a proven component.
- Treat this as compatibility mode.

---

### 4.1.13 AetherDesk Stream Engine

This is the strategic high-performance remote GUI engine.

Responsibilities:

- Adaptive display streaming
- Region damage tracking
- Tile-based caching
- Glyph and image caching
- Mixed lossless/lossy encoding
- Hardware-accelerated decode
- Input prioritization
- Bandwidth estimation
- Frame pacing
- Session resume
- Clipboard and window metadata synchronization
- Optional audio later

Protocol name suggestion: **ARPP — Adaptive Remote Presentation Protocol**

---

### 4.1.14 Plugin Host

Responsibilities:

- Load plugins
- Sandbox plugin execution
- Provide plugin APIs
- Manage plugin lifecycle
- Enforce permissions
- Provide UI extension points

Plugin types:

- Protocol providers
- Terminal shell providers
- File system providers
- Credential providers
- UI panels
- Command palette commands
- Session hooks
- Automation scripts
- Theme providers

Sandbox strategy:

- WASM for lightweight sandboxed plugins
- Native plugins only through a separate broker process with explicit permissions
- No direct access to credential vault unless approved

---

### 4.1.15 Diagnostics and Telemetry

Responsibilities:

- Structured logs
- Latency metrics
- Bandwidth metrics
- Frame statistics
- Error reports
- Session health indicators
- Optional opt-in telemetry
- Redaction of secrets

---

## 4.2 Remote Linux Agent Components

The Linux agent is essential for X2Go-class or better application publishing.

### 4.2.1 Agent Supervisor

Responsibilities:

- Start/stop user sessions
- Authenticate against SSH session
- Enforce session isolation
- Report health
- Restart failed components
- Clean up temporary resources

---

### 4.2.2 Wayland Compositor / Surface Host

Responsibilities:

- Host remote applications in a controlled Wayland environment
- Support XWayland for legacy X11 apps
- Track surfaces, focus, resize, and window metadata
- Capture buffers efficiently
- Support GPU buffers where available
- Support headless operation

Possible implementation:

- Rust compositor using Smithay
- Or integration with existing headless compositor technology
- Avoid requiring root

---

### 4.2.3 Input Engine

Responsibilities:

- Receive keyboard/mouse/touch input from client
- Translate keymaps
- Inject input via uinput/libinput or compositor APIs
- Preserve modifiers and keyboard layout as much as possible
- Support clipboard shortcuts safely

---

### 4.2.4 Clipboard Bridge

Responsibilities:

- Synchronize clipboard between client and remote apps
- Handle text, images, and files where safe
- Prevent accidental data exfiltration based on policy
- Support selection clipboard for X11 compatibility

---

### 4.2.5 Encoder Pipeline

Responsibilities:

- Detect dirty regions
- Classify regions: text/UI, image, video
- Choose encoding mode:
  - Lossless for text and UI
  - JPEG/WebP/AVIF-like stills for complex images
  - H.264/AV1 for motion/video regions
- Use GPU encoding when available:
  - VAAPI
  - NVENC
  - QSV
- Fallback to software encoding

---

### 4.2.6 Transport Adapter

Responsibilities:

- Send control/input/display messages
- Prioritize input and control
- Support SSH channel transport
- Support optional UDP/QUIC/DTLS media path
- Perform bandwidth estimation
- Support FEC or retransmission policy depending on mode
- Provide metrics

---

### 4.2.7 File and Printing Bridge

Responsibilities:

- Optional file exchange between client and remote app sandbox
- Print forwarding later
- Drag-and-drop policy enforcement

---

## 4.3 Transport Design

### 4.3.1 SSH-First Transport

All sessions should be able to operate over SSH only.

Advantages:

- Firewall-friendly
- Familiar security model
- No additional server ports required
- Easy bootstrap through existing SSH access

Challenges:

- TCP head-of-line blocking
- Channel flow-control overhead
- Latency amplification under loss
- Less ideal for high-frequency media

Mitigations:

- Separate channels for control, input, display, clipboard, and file transfer
- Disable Nagle where appropriate
- Use compression selectively
- Prioritize input/control channels
- Avoid sending bulk display data over the same channel as interactive input when possible
- Use session multiplexing and connection reuse

---

### 4.3.2 Optional Secure Direct Media Path

For better WAN performance, AetherDesk should support an optional direct media path.

Design:

1. SSH session authenticates and authorizes media session.
2. Remote agent opens ephemeral UDP port or uses QUIC listener.
3. Server sends short-lived signed token to client over SSH.
4. Client connects using DTLS 1.3 or QUIC with token.
5. Media path is used only if permitted by policy and network.
6. If UDP is blocked, fallback automatically to SSH channel.

Security requirements:

- Token bound to SSH session ID
- Short TTL
- Mutual authentication
- Encrypted transport
- Replay protection
- User-visible indicator when direct media path is active

This is optional but highly recommended for beating X2Go on WAN.

---

## 4.4 Adaptive Remote Presentation Protocol Design

### 4.4.1 Goals

- Low added latency over SSH or QUIC
- Good text sharpness
- Good image quality
- Efficient video playback
- Robustness to packet loss and bandwidth variation
- Session resume
- Minimal remote agent footprint
- Secure by default

---

### 4.4.2 Message Classes

| Class | Examples | Priority |
|---|---|---:|
| Control | session start, capabilities, resize, window map/unmap | High |
| Input | keyboard, mouse, touch, focus | Highest |
| Clipboard | clipboard sync, drag-and-drop metadata | High |
| Display | frame updates, damage regions, cache operations | Medium |
| Audio | audio samples | Medium |
| Metrics | latency probes, frame stats | Low |
| Bulk | file transfer for app bridge | Low |

Input must never be queued behind large display updates.

---

### 4.4.3 Display Update Strategy

1. Track damaged regions.
2. Coalesce nearby regions.
3. Classify regions:
   - Text/UI
   - Image
   - Video/motion
4. Select encoding:
   - Lossless tile for text/UI
   - JPEG/WebP for complex stills
   - H.264/AV1 for video regions
5. Use tile cache and hash-based deduplication.
6. Send only changed tiles or references to cached tiles.
7. Use cursor as separate low-latency layer where possible.
8. Frame pacing adjusts to RTT, loss, and bandwidth estimate.

---

### 4.4.4 Caching

Cache types:

- Glyph cache
- Tile cache
- Image hash cache
- Window thumbnail cache
- Palette cache
- Compression dictionary cache
- SSH connection/session cache

Cache invalidation must be deterministic and secure.

---

### 4.4.5 Bandwidth Adaptation

Inputs to adaptation:

- RTT samples
- Loss rate
- Throughput estimate
- Frame queue depth
- Input lag
- CPU/GPU load
- User-selected quality preference

Modes:

- Sharp: favor lossless/text quality
- Balanced: mixed adaptive
- Smooth: favor frame rate
- Low bandwidth: aggressive compression, lower FPS
- Custom: user-defined caps

---

## 4.5 Security Design

### 4.5.1 Authentication

- SSH password
- SSH public key
- SSH certificates
- Keyboard-interactive
- Hardware key support for app unlock and future SSH key handling
- Optional MFA for credential vault

### 4.5.2 Host Trust

- Known hosts database
- Host key fingerprints
- Certificate authority trust
- Host key change warnings
- Optional pinning for sensitive sessions

### 4.5.3 Secrets

- Secrets stored in OS credential store by default
- Optional encrypted vault with Argon2id key derivation and AES-GCM
- Secrets never logged
- Memory zeroization where possible
- Plugin access requires explicit permission

### 4.5.4 Plugin Security

- Manifest-declared permissions
- Sandboxed WASM runtime for lightweight plugins
- Native plugins isolated in separate process
- No direct access to SSH channels unless granted
- User consent prompts for sensitive operations

### 4.5.5 Remote Agent Security

- Agent runs as user, not root
- Sessions isolated per user
- Temporary files cleaned up
- No privileged input injection beyond session scope
- Clipboard and file transfer policy controlled
- Agent binary signed and version-checked

---

## 4.6 Performance Design

### 4.6.1 Startup Optimization

- Native binary launcher
- Lazy load plugins
- Avoid runtime JIT in hot startup path
- Precompile shaders
- Cache session tree and UI state
- Delay nonessential services until first frame
- Use session restore instead of full reconnect where possible

### 4.6.2 CPU Optimization

- GPU text rendering
- Zero-copy display paths where possible
- Avoid double compression
- Use zstd/lz4 selectively
- Batch UI updates
- Keep UI thread separate from protocol threads
- Use thread pools only where necessary

### 4.6.3 RAM Optimization

- Compact terminal scrollback storage
- Memory-mapped scrollback for large histories
- Tile cache limits
- Image cache eviction
- Plugin lazy loading
- Optional process isolation only for high-risk components

### 4.6.4 Latency Optimization

- Input messages sent immediately
- Display updates coalesced but not delayed excessively
- Separate control/input queues
- TCP_NODELAY where appropriate
- Optional UDP/QUIC media path
- Frame pacing with minimal jitter buffer
- Predictive cursor rendering where useful

---

## 5. Software Module Definitions

| Module ID | Module | Responsibility | Primary Technology |
|---|---|---|---|
| MOD-UI-01 | App Shell | Main window, tabs, panes, command palette | Flutter/Dart |
| MOD-UI-02 | Session Tree UI | Browse/search/launch sessions | Flutter |
| MOD-UI-03 | Settings UI | Preferences and profiles | Flutter |
| MOD-CORE-01 | Core Broker | Message bus, session orchestration | Rust |
| MOD-CORE-02 | Config Store | TOML/JSON config, profiles | Rust |
| MOD-CORE-03 | Event Bus | Async pub/sub events | Rust |
| MOD-SSH-01 | SSH Client | SSH protocol and channels | Rust/OpenSSH integration |
| MOD-SSH-02 | Auth Manager | Password/key/cert auth | Rust |
| MOD-SSH-03 | Known Hosts | Host trust management | Rust |
| MOD-SSH-04 | Tunnel Manager | Local/remote/dynamic forwarding | Rust |
| MOD-SSH-05 | SFTP Client | File operations over SSH | Rust |
| MOD-TERM-01 | PTY Backend | Local ConPTY/POSIX PTY | Rust |
| MOD-TERM-02 | VT Parser | Terminal state machine | Rust |
| MOD-TERM-03 | Terminal Renderer | GPU text rendering | Rust + wgpu/D3D |
| MOD-TERM-04 | Scrollback Store | Efficient history storage | Rust |
| MOD-FILE-01 | File Browser UI | Local/remote file browser | Flutter |
| MOD-FILE-02 | Transfer Queue | Parallel/resumable transfers | Rust |
| MOD-RDP-01 | RDP Client Engine | RDP connection/rendering | FreeRDP + Rust wrapper |
| MOD-VNC-01 | VNC Client Engine | RFB connection/rendering | Rust/C library |
| MOD-X11-01 | X Server Bridge | Manage local X server | Rust + external X server |
| MOD-X11-02 | X11 Forwarding Adapter | SSH X11 channel handling | Rust |
| MOD-STREAM-01 | Stream Protocol | ARPP framing/messages | Rust |
| MOD-STREAM-02 | Display Decoder | Client-side decode | Rust + C codecs |
| MOD-STREAM-03 | Display Renderer | Present frames/textures | Rust + GPU API |
| MOD-STREAM-04 | QoS Controller | Adaptive bitrate/latency | Rust |
| MOD-AGENT-01 | Linux Agent Supervisor | Start/stop sessions | Rust |
| MOD-AGENT-02 | Wayland Host | App hosting/compositor | Rust/Smithay or C |
| MOD-AGENT-03 | Input Injector | Keyboard/mouse input | Rust/uinput |
| MOD-AGENT-04 | Clipboard Bridge | Clipboard sync | Rust |
| MOD-AGENT-05 | Encoder Pipeline | Region encoding | Rust + GStreamer/VAAPI |
| MOD-PLUGIN-01 | Plugin Host | Load/run plugins | Rust/Wasmtime |
| MOD-PLUGIN-02 | Plugin API | Stable APIs for plugins | Rust/FFI/IPC |
| MOD-SEC-01 | Credential Vault | Secrets storage | Rust + OS keychain |
| MOD-SEC-02 | Policy Engine | Permission checks | Rust |
| MOD-OBS-01 | Logger | Structured logs | Rust |
| MOD-OBS-02 | Metrics | Latency/CPU/bandwidth stats | Rust |

---

## 6. Recommended Languages and Frameworks

### 6.1 Primary Recommendation

| Layer | Recommended Technology | Reason |
|---|---|---|
| Core engine | Rust | Memory safety, async I/O, performance, cross-platform potential |
| Terminal parser/renderer | Rust | High performance and safe parsing |
| SSH/SFTP/tunnels | Rust crates or OpenSSH integration | Strong ecosystem and control |
| UI | Flutter Desktop | Modern UI, fast development, GPU rendering |
| RDP | FreeRDP via C ABI | Mature RDP implementation |
| VNC | Rust or C RFB client | Simpler protocol, can be in-house |
| Codecs | C/C++ libraries + hardware APIs | Mature ecosystem |
| Plugin sandbox | Wasmtime | Secure sandboxing |
| Config | TOML + JSON | Human-editable and machine-friendly |
| CI/CD | GitHub Actions / Azure DevOps | Cross-platform builds |

### 6.2 UI Alternative

If Flutter Desktop is considered too risky, alternatives are:

| Option | Pros | Cons |
|---|---|---|
| Qt/QML | Mature desktop stack, strong rendering | Licensing complexity, C++ heaviness |
| Avalonia | .NET cross-platform, good controls | Interop with Rust core adds complexity |
| Slint | Lightweight native feel | Ecosystem smaller than Flutter/Qt |
| WinUI/WPF + Avalonia later | Best Windows-native feel | Cross-platform delayed |

For a small team wanting modern UI and cross-platform potential, Flutter is recommended, but the terminal renderer should be custom-rendered into textures or surfaces rather than relying purely on Flutter text widgets for extreme throughput.

---

## 7. MVP Roadmap

## 7.1 MVP Definition

The MVP should prove:

- Fast, stable SSH terminal experience
- Reliable SFTP
- Session and credential management
- Basic SSH tunneling
- Modern tabbed UI
- Low resource usage
- Extensible architecture for later GUI engines

Remote GUI parity with X2Go is a later milestone, not the first MVP.

---

## 7.2 Phase 0 — Foundation

Duration: 4–6 weeks

Deliverables:

- Repository structure
- CI/CD pipelines
- Rust core skeleton
- Flutter shell skeleton
- Logging and crash reporting
- Config store
- Basic session data model
- Secure storage spike
- SSH library selection validated
- Terminal rendering spike

Exit criteria:

- App launches in under 2 seconds on target machine
- Rust-Flutter bridge works reliably
- Basic tabbed shell can host placeholder views
- Secure storage proof-of-concept works on Windows

---

## 7.3 Phase 1 — SSH Terminal MVP

Duration: 10–14 weeks

Deliverables:

- SSH connection manager
- Password and public key authentication
- Known hosts handling
- Multi-tab terminal
- Basic split panes
- VT parser compatibility with common CLI apps
- Scrollback
- Copy/paste
- Basic session manager
- Credential integration with Windows Credential Manager
- Basic settings page

Exit criteria:

- Connect to OpenSSH servers reliably
- Terminal passes core VT test suite
- Keystroke latency acceptable
- No secret leakage in logs
- Stable across 8-hour soak test

---

## 7.4 Phase 2 — SFTP, Tunnels, Session Polish

Duration: 8–12 weeks

Deliverables:

- SFTP file browser
- Transfer queue
- Resume support
- Local/remote port forwarding
- Dynamic SOCKS proxy
- Session folders/tags/search
- Reconnect logic
- Jump host support
- Command palette
- Layout save/restore

Exit criteria:

- File transfers stable under poor network conditions
- Tunnels survive reconnects where policy allows
- Session manager handles hundreds of sessions smoothly
- Startup remains under 2 seconds

---

## 7.5 Phase 3 — GUI Protocol Baseline

Duration: 10–14 weeks

Deliverables:

- RDP viewer using FreeRDP
- VNC viewer
- Managed X11 bridge compatibility mode
- Display orchestrator
- Basic clipboard support for RDP/VNC where feasible
- Session types for RDP/VNC/X11

Exit criteria:

- RDP connections work against common Windows targets
- VNC connections work against common servers
- X11 forwarding works with managed local X server path
- Display views integrate into tabs/panes

---

## 7.6 Phase 4 — Linux Application Streaming Alpha

Duration: 16–24 weeks

Deliverables:

- Linux agent installer over SSH
- Agent supervisor
- Headless Wayland/XWayland app session
- Basic input injection
- Basic clipboard
- Adaptive streaming alpha
- Remote app window mapping
- Performance telemetry

Exit criteria:

- Run at least a set of reference Linux apps remotely
- Interactive latency competitive with X2Go on test WAN profiles
- Session recovery works
- No privilege escalation from agent

---

## 7.7 Phase 5 — Hardening and v1.0

Duration: 10–14 weeks

Deliverables:

- Plugin SDK preview
- Credential vault hardening
- Performance tuning
- Crash resilience
- Installer and auto-update
- Documentation
- Security review
- Beta feedback loop

Exit criteria:

- Crash-free rate above target
- Startup target met
- Memory target met
- Core workflows stable
- Security review passed

---

## 8. Project Folder Structure

```text
aetherdesk/
├── README.md
├── LICENSE
├── docs/
│   ├── architecture/
│   ├── requirements/
│   ├── security/
│   ├── protocols/
│   └── runbooks/
├── scripts/
│   ├── build/
│   ├── package/
│   ├── test/
│   └── release/
├── ci/
│   ├── pipelines/
│   └── lint/
├── core/
│   ├── rust/
│   │   ├── Cargo.toml
│   │   ├── crates/
│   │   │   ├── ad-core/
│   │   │   ├── ad-config/
│   │   │   ├── ad-events/
│   │   │   ├── ad-session/
│   │   │   ├── ad-credential/
│   │   │   ├── ad-plugin-host/
│   │   │   ├── ad-telemetry/
│   │   │   ├── ad-ssh/
│   │   │   ├── ad-sftp/
│   │   │   ├── ad-tunnel/
│   │   │   ├── ad-terminal/
│   │   │   ├── ad-terminal-render/
│   │   │   ├── ad-display/
│   │   │   ├── ad-stream-client/
│   │   │   ├── ad-rdp-bridge/
│   │   │   ├── ad-vnc/
│   │   │   └── ad-x11-bridge/
│   │   └── tests/
│   └── ffi/
│       ├── c-api/
│       └── flutter-bridge/
├── ui/
│   ├── flutter_app/
│   │   ├── lib/
│   │   │   ├── main.dart
│   │   │   ├── app/
│   │   │   ├── core/
│   │   │   ├── features/
│   │   │   │   ├── terminal/
│   │   │   │   ├── sftp/
│   │   │   │   ├── sessions/
│   │   │   │   ├── tunnels/
│   │   │   │   ├── rdp/
│   │   │   │   ├── vnc/
│   │   │   │   ├── x11/
│   │   │   │   ├── remote_apps/
│   │   │   │   ├── settings/
│   │   │   │   └── plugins/
│   │   │   └── widgets/
│   │   ├── windows/
│   │   ├── linux/
│   │   ├── macos/
│   │   └── test/
│   └── design/
│       ├── tokens/
│       └── mockups/
├── protocols/
│   ├── arpp/
│   │   ├── spec/
│   │   ├── schema/
│   │   └── conformance/
│   ├── rdp/
│   ├── vnc/
│   └── x11/
├── agents/
│   ├── linux-agent/
│   │   ├── Cargo.toml
│   │   ├── crates/
│   │   │   ├── agent-supervisor/
│   │   │   ├── agent-session/
│   │   │   ├── agent-wayland/
│   │   │   ├── agent-input/
│   │   │   ├── agent-clipboard/
│   │   │   ├── agent-encoder/
│   │   │   └── agent-transport/
│   │   └── packaging/
│   └── windows-agent/
│       └── placeholder/
├── plugins/
│   ├── sdk/
│   ├── examples/
│   └── sandbox-policy/
├── third_party/
│   ├── freerdp/
│   ├── codecs/
│   └── licenses/
├── test/
│   ├── e2e/
│   ├── performance/
│   ├── fuzz/
│   ├── network-emulation/
│   └── compatibility/
└── installer/
    ├── windows/
    ├── linux/
    └── macos/
```

---

## 9. Highest-Risk Technical Challenges

| Risk | Severity | Why It Is Hard | Mitigation |
|---|---:|---|---|
| Full X11 server on Windows | Very high | X11 server implementation is huge and legacy-heavy | Use managed external X server for MVP; treat X11 as compatibility mode |
| Remote Linux app publishing | Very high | Wayland fragmentation, input, clipboard, GPU, sandboxing | Build controlled agent environment; start with limited app set |
| Beating X2Go on WAN | High | Requires adaptive encoding, caching, and robust QoS | Prototype ARPP early; benchmark against X2Go continuously |
| SSH transport head-of-line blocking | High | TCP over SSH can stall display/input | Separate channels, optional QUIC/UDP media path, input prioritization |
| Terminal compatibility | High | VT/xterm edge cases are numerous | Use established terminal parser/test suites; fuzz heavily |
| FreeRDP integration complexity | Medium | Native library integration, rendering, threading | Isolate behind stable bridge; use proven wrapper patterns |
| Credential security | High | Secrets are high-value targets | OS credential store, memory hygiene, no plugin access by default |
| Plugin sandbox escape | High | Plugins can become attack surface | WASM sandbox, native plugin isolation, permission prompts |
| Hardware codec licensing | Medium | H.264/AV1 patents and distribution concerns | Legal review, optional codecs, use open alternatives where possible |
| Fast startup while feature-rich | Medium | Feature bloat can destroy startup time | Lazy loading, modular startup, profiling gates |
| Cross-platform GPU rendering | Medium | Driver differences and texture interop | Abstract renderer, test on common GPUs, fallback paths |
| Agent installation across distros | High | Dependencies and permissions vary | Static Rust agent, minimal deps, distro-specific test matrix |

---

## 10. Development Complexity Estimate for a Small AI-Assisted Team

Assumed team:

- 1 technical lead / architect
- 2 systems engineers, Rust/core
- 1 UI engineer, Flutter/desktop
- 1 protocol/media engineer
- Shared QA/DevOps support
- AI assistance for boilerplate, tests, docs, reviews

AI assistance can accelerate development, but it does not remove systems-level integration risk. The hardest parts are protocol correctness, Linux compatibility, GPU pipelines, and security.

### 10.1 Estimate by Module

| Workstream | Person-Months | Notes |
|---|---:|---|
| Core shell/config/events | 3–5 | Foundation |
| Terminal engine | 7–10 | High compatibility burden |
| SSH/SFTP/tunnels | 6–9 | Depends on library choice |
| Credential/session manager | 3–5 | Security-sensitive |
| Flutter UI | 7–10 | Tabs, panes, settings, file browser |
| RDP integration | 3–6 | FreeRDP wrapper |
| VNC integration | 2–4 | Simpler |
| X11 bridge compatibility | 4–8 | Lower if external X server used |
| Stream protocol alpha | 8–12 | Core differentiator |
| Linux agent alpha | 10–16 | Highest uncertainty |
| Plugin architecture | 4–7 | Sandbox and SDK |
| QA/CI/performance | 6–10 | Continuous investment |
| Security review/hardening | 3–6 | Must not be skipped |

Total rough range:

- **Lean MVP without advanced Linux app streaming:** 25–40 person-months
- **Full v1 with Linux app streaming alpha:** 60–100 person-months

With a 4–5 person team and AI assistance:

- **MVP SSH productivity release:** 6–9 months
- **GUI protocol baseline release:** 10–14 months
- **v1 with Linux app streaming:** 16–24 months

AI may reduce some module costs by 20–40%, but not uniformly. The highest-risk systems work remains expensive.

---

# Part II — Technical Requirements Document

---

# AetherDesk Technical Requirements Document

## 1. Scope

This TRD defines technical requirements for AetherDesk, a cross-platform remote-workspace client with primary support for Windows.

The product includes:

- SSH terminal client
- SFTP file browser
- SSH tunneling
- RDP client
- VNC client
- X11 forwarding compatibility
- Remote Linux application publishing
- Session management
- Credential management
- Plugin architecture
- Performance-optimized remote GUI streaming

---

## 2. Technical Goals

| Goal | Requirement |
|---|---|
| Startup performance | Cold start under 2 seconds on reference hardware |
| Memory usage | Base client under 250–350 MB RAM with no active heavy sessions |
| CPU usage | Idle CPU under 2%; terminal scrolling efficient |
| Terminal latency | Local added latency under 20 ms where practical |
| Remote GUI latency | Added protocol latency target under 50 ms on 100–200 ms RTT links, excluding network RTT |
| Bandwidth adaptation | Operate from low-bandwidth SSH-like conditions up to 100 Mbps LAN |
| Reliability | Automatic reconnect for transient failures where session policy allows |
| Security | Secrets protected at rest and in memory; plugin sandboxing |
| Maintainability | Modular crates/packages with clear API boundaries |

---

## 3. Platform Requirements

### 3.1 Client Platforms

Primary:

- Windows 10/11 x64

Future:

- Linux desktop
- macOS

### 3.2 Remote Targets

- Linux servers with OpenSSH
- Windows machines with RDP
- VNC servers
- Linux hosts capable of running AetherDesk agent
- WSL environments where applicable

---

## 4. Functional Technical Requirements

### 4.1 SSH Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-SSH-001 | Support SSHv2 client connections | Must |
| TR-SSH-002 | Support password, public key, keyboard-interactive auth | Must |
| TR-SSH-003 | Support SSH certificates | Should |
| TR-SSH-004 | Support known hosts and host key verification | Must |
| TR-SSH-005 | Support jump hosts / proxy commands | Must |
| TR-SSH-006 | Support PTY allocation and environment variables | Must |
| TR-SSH-007 | Support SSH keepalive and rekeying | Must |
| TR-SSH-008 | Support connection multiplexing/reuse | Should |
| TR-SSH-009 | Support SSH agent forwarding with user consent | Should |
| TR-SSH-010 | Support compression negotiation | Should |

---

### 4.2 Terminal Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-TERM-001 | Multi-tab terminal | Must |
| TR-TERM-002 | Split panes | Must |
| TR-TERM-003 | VT100/xterm-256color compatibility | Must |
| TR-TERM-004 | True color support | Must |
| TR-TERM-005 | Unicode and wide-character support | Must |
| TR-TERM-006 | Scrollback buffer with search | Must |
| TR-TERM-007 | Copy/paste with Windows clipboard | Must |
| TR-TERM-008 | Mouse reporting | Should |
| TR-TERM-009 | Hyperlinks | Should |
| TR-TERM-010 | GPU-accelerated rendering | Should |
| TR-TERM-011 | Local ConPTY terminal support | Should |
| TR-TERM-012 | Terminal profiles | Must |

---

### 4.3 SFTP Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-SFTP-001 | Remote file browsing over SFTP | Must |
| TR-SFTP-002 | Local file browsing | Must |
| TR-SFTP-003 | Upload/download with progress | Must |
| TR-SFTP-004 | Transfer queue | Must |
| TR-SFTP-005 | Resume interrupted transfers | Should |
| TR-SFTP-006 | Parallel transfers | Should |
| TR-SFTP-007 | Permission editing | Should |
| TR-SFTP-008 | Symlink display/handling | Should |
| TR-SFTP-009 | File hash calculation | Could |
| TR-SFTP-010 | Remote file edit handoff | Could |

---

### 4.4 Tunneling Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-TUN-001 | Local port forwarding | Must |
| TR-TUN-002 | Remote port forwarding | Should |
| TR-TUN-003 | Dynamic SOCKS5 proxy | Should |
| TR-TUN-004 | Tunnel status monitoring | Must |
| TR-TUN-005 | Automatic tunnel reconnect | Should |
| TR-TUN-006 | Per-session tunnel definitions | Must |
| TR-TUN-007 | Port conflict handling | Must |

---

### 4.5 RDP Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-RDP-001 | Connect to standard RDP servers | Must |
| TR-RDP-002 | Support username/password and NLA | Must |
| TR-RDP-003 | Support clipboard | Should |
| TR-RDP-004 | Support drive redirection | Could |
| TR-RDP-005 | Support RemoteApp where feasible | Could |
| TR-RDP-006 | Hardware-accelerated decoding where available | Should |
| TR-RDP-007 | Save RDP session profiles | Must |

---

### 4.6 VNC Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-VNC-001 | Connect to standard RFB servers | Must |
| TR-VNC-002 | Support common auth types | Must |
| TR-VNC-003 | Support Tight/ZRLE encodings | Should |
| TR-VNC-004 | Support cursor handling | Should |
| TR-VNC-005 | Support clipboard where server supports | Could |
| TR-VNC-006 | Support scaling | Should |

---

### 4.7 X11 Forwarding Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-X11-001 | Support SSH X11 forwarding channels | Must |
| TR-X11-002 | Manage local display configuration | Must |
| TR-X11-003 | Manage xauth/MIT-MAGIC-COOKIE | Must |
| TR-X11-004 | Integrate with local X server process | Must |
| TR-X11-005 | Support trusted/untrusted mode policy | Should |
| TR-X11-006 | Do not require full custom X server in MVP | Must |

---

### 4.8 Remote Linux Application Publishing Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-APP-001 | Deploy/start Linux agent over SSH | Must for v1 GUI release |
| TR-APP-002 | Launch remote Linux app in isolated session | Must |
| TR-APP-003 | Display remote app window in client | Must |
| TR-APP-004 | Forward keyboard/mouse input | Must |
| TR-APP-005 | Clipboard synchronization | Should |
| TR-APP-006 | Session resume/reconnect | Should |
| TR-APP-007 | XWayland support for legacy apps | Should |
| TR-APP-008 | GPU acceleration where available | Could |
| TR-APP-009 | Audio forwarding | Could |
| TR-APP-010 | Per-app launch catalog | Should |

---

### 4.9 Session Manager Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-SESS-001 | Sessions stored in folders/tags | Must |
| TR-SESS-002 | Search sessions | Must |
| TR-SESS-003 | Session templates | Should |
| TR-SESS-004 | Save layouts | Should |
| TR-SESS-005 | Restore previous workspace | Should |
| TR-SESS-006 | Session health indicators | Should |
| TR-SESS-007 | Session notes/metadata | Could |

---

### 4.10 Credential Manager Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-CRED-001 | Store credentials securely | Must |
| TR-CRED-002 | Use Windows Credential Manager by default | Must |
| TR-CRED-003 | Optional encrypted vault | Should |
| TR-CRED-004 | Support SSH keys and passphrases | Must |
| TR-CRED-005 | Never log secrets | Must |
| TR-CRED-006 | Plugin access requires explicit permission | Must |

---

### 4.11 Plugin Requirements

| ID | Requirement | Priority |
|---|---|---|
| TR-PLUG-001 | Plugin manifest with permissions | Must |
| TR-PLUG-002 | Sandboxed plugin runtime | Must |
| TR-PLUG-003 | Command palette extension API | Should |
| TR-PLUG-004 | Session lifecycle hooks | Should |
| TR-PLUG-005 | Protocol provider API | Could |
| TR-PLUG-006 | UI panel API | Could |
| TR-PLUG-007 | Native plugin isolation | Should if native plugins supported |

---

## 5. Non-Functional Requirements

### 5.1 Performance

| ID | Requirement |
|---|---|
| TR-NFR-PERF-001 | Cold startup < 2 seconds on reference Windows machine |
| TR-NFR-PERF-002 | Warm startup < 800 ms |
| TR-NFR-PERF-003 | Base RAM < 350 MB without heavy sessions |
| TR-NFR-PERF-004 | Terminal tab idle CPU < 1–2% |
| TR-NFR-PERF-005 | Terminal scrolling should maintain smooth frame pacing |
| TR-NFR-PERF-006 | Remote GUI input messages must be prioritized over bulk display updates |
| TR-NFR-PERF-007 | Client should support hardware-accelerated video decode where available |
| TR-NFR-PERF-008 | SFTP directory listing for 1,000 files should render within 1 second under normal network conditions |

---

### 5.2 Reliability

| ID | Requirement |
|---|---|
| TR-NFR-REL-001 | Application must not crash on malformed terminal output |
| TR-NFR-REL-002 | Network disconnection must not corrupt session state |
| TR-NFR-REL-003 | File transfers must be resumable where protocol supports |
| TR-NFR-REL-004 | Plugin crash must not bring down entire client if sandboxed |
| TR-NFR-REL-005 | Reconnect policies must be user-configurable |

---

### 5.3 Security

| ID | Requirement |
|---|---|
| TR-NFR-SEC-001 | Secrets must be encrypted at rest |
| TR-NFR-SEC-002 | Host key verification must be enforced |
| TR-NFR-SEC-003 | Insecure credential fallback must require explicit user action |
| TR-NFR-SEC-004 | Crash dumps must be scrubbed of secrets where feasible |
| TR-NFR-SEC-005 | Plugin permissions must be explicit |
| TR-NFR-SEC-006 | Remote agent must not require root for standard app sessions |

---

### 5.4 Maintainability

| ID | Requirement |
|---|---|
| TR-NFR-MAINT-001 | Core modules must have unit tests |
| TR-NFR-MAINT-002 | Protocol parsers must be fuzzed |
| TR-NFR-MAINT-003 | Public APIs between modules must be versioned |
| TR-NFR-MAINT-004 | Feature flags must allow disabling experimental engines |
| TR-NFR-MAINT-005 | Logging must be structured |

---

## 6. Protocol Requirements

### 6.1 ARPP Protocol Requirements

| ID | Requirement |
|---|---|
| TR-ARPP-001 | Must support capability negotiation |
| TR-ARPP-002 | Must separate control, input, display, clipboard, and bulk channels |
| TR-ARPP-003 | Must support damage-region updates |
| TR-ARPP-004 | Must support tile/image caching |
| TR-ARPP-005 | Must support adaptive encoding selection |
| TR-ARPP-006 | Must support session resume |
| TR-ARPP-007 | Must support SSH channel transport |
| TR-ARPP-008 | Should support QUIC/DTLS direct media transport |
| TR-ARPP-009 | Must support latency probing |
| TR-ARPP-010 | Must support encryption for all transports |

---

## 7. Data Requirements

### 7.1 Session Data

Session record fields:

- Unique ID
- Name
- Type
- Host
- Port
- Username
- Auth method reference
- Proxy/jump configuration
- Environment variables
- Startup commands
- Tunnel definitions
- Display settings
- Tags/folders
- Last-used timestamp
- Health metrics
- User notes

### 7.2 Credential Data

- Credential ID
- Type: password, key, certificate, token
- Secret reference or OS vault handle
- Metadata
- Created/updated timestamps
- Usage restrictions

### 7.3 Plugin Data

- Plugin ID
- Version
- Permissions
- Entry point
- Sandbox policy
- Enabled/disabled state

---

## 8. Interface Requirements

### 8.1 UI-to-Core API

The UI should communicate with the core through a stable asynchronous API.

Example operations:

- `list_sessions`
- `create_session`
- `connect_session`
- `disconnect_session`
- `open_terminal_tab`
- `open_sftp_view`
- `create_tunnel`
- `list_credentials`
- `request_credential_access`
- `install_remote_agent`
- `launch_remote_app`
- `get_metrics`

### 8.2 Core-to-Engine API

Engines should implement common traits such as:

- `Connect`
- `Disconnect`
- `Reconnect`
- `HandleInput`
- `Resize`
- `RenderSurface`
- `GetMetrics`
- `ExportClipboard`
- `ImportClipboard`

---

## 9. Observability Requirements

| ID | Requirement |
|---|---|
| TR-OBS-001 | Structured logs with correlation IDs |
| TR-OBS-002 | Session-level metrics: RTT, throughput, FPS, frame drops |
| TR-OBS-003 | Terminal throughput metrics for diagnostics |
| TR-OBS-004 | Optional user-consented telemetry |
| TR-OBS-005 | Redaction of credentials and tokens |
| TR-OBS-006 | Exportable diagnostics bundle |

---

## 10. Packaging and Deployment Requirements

| ID | Requirement |
|---|---|
| TR-DEP-001 | Windows installer: MSIX and/or MSI |
| TR-DEP-002 | Portable mode should be supported where feasible |
| TR-DEP-003 | Signed binaries |
| TR-DEP-004 | Auto-update with rollback |
| TR-DEP-005 | Linux agent deployable as single binary where possible |
| TR-DEP-006 | Agent version compatibility checks |

---

## 11. Testing Requirements

| Area | Requirement |
|---|---|
| Unit tests | Core logic, parsers, session manager |
| Integration tests | SSH, SFTP, tunnels against local test servers |
| Protocol fuzzing | Terminal parser, ARPP messages, VNC/RDP bridge inputs |
| UI tests | Critical workflows |
| Performance tests | Startup, memory, CPU, terminal throughput |
| Network emulation | High latency, loss, bandwidth limits |
| Compatibility tests | OpenSSH versions, distros, Windows RDP targets |
| Security tests | Credential storage, plugin sandbox, host key handling |

---

## 12. Licensing and Compliance Considerations

- Prefer permissive licenses for core components.
- Review FreeRDP license obligations.
- Review codec licensing for H.264/AV1/JPEG-related patents.
- Avoid bundling GPL components into proprietary core unless isolated and legally reviewed.
- Maintain third-party license inventory.
- Provide notice for cryptographic dependencies.

---

# Part III — Product Requirements Document

---

# AetherDesk Product Requirements Document

## 1. Product Vision

AetherDesk is a fast, secure, modern remote-workspace client for engineers, operators, and administrators who need one tool for:

- SSH terminal access
- File transfer
- Tunneling
- Remote desktop access
- Remote Linux application publishing

The product should feel lighter and faster than traditional tool suites while providing enterprise-grade session and credential management.

---

## 2. Target Users

### 2.1 DevOps / Platform Engineer

Needs:

- SSH to many servers
- Port forwarding
- SFTP
- Tmux/CLI workflows
- Reliable reconnects

### 2.2 System Administrator

Needs:

- RDP to Windows servers
- VNC to legacy systems
- Session organization
- Credential management

### 2.3 Application Developer

Needs:

- Remote Linux GUI apps
- Low-latency editing/debugging
- File synchronization
- Multi-tab workflow

### 2.4 Support / Operations Engineer

Needs:

- Many saved sessions
- Search
- Quick access
- Session notes
- Stable remote tools

---

## 3. Problem Statement

Existing tools are often:

- Too heavy
- Slow to start
- Poor on WAN for Linux GUI apps
- Fragmented across terminal, SFTP, RDP, VNC, and X forwarding
- Weak in session/credential organization
- Limited in plugin extensibility

AetherDesk should consolidate these workflows into one performant product.

---

## 4. Product Goals

| Goal | Success Indicator |
|---|---|
| Fast startup | Users open app and connect within seconds |
| Low resource usage | Users keep app open all day |
| Strong SSH productivity | Terminal/SFTP/tunnels become primary daily tool |
| Competitive remote GUI | Users prefer AetherDesk over X2Go for supported apps |
| Session organization | Users manage hundreds of sessions easily |
| Secure credential handling | No plaintext secrets stored by default |
| Extensibility | Plugins can safely extend workflows |

---

## 5. Non-Goals for Initial Release

To keep scope manageable, initial releases will not focus on:

- Full replacement for all X server extensions
- Full NoMachine NX compatibility
- Mobile clients
- Web-based client
- Full remote audio/video collaboration
- Windows RemoteApp publishing server
- Full configuration management/deployment tooling
- Multi-user team synchronization service

---

## 6. Core Product Features

## 6.1 Multi-Tab Terminal

User story:

> As an engineer, I want multiple SSH terminal tabs and split panes so I can work across servers efficiently.

Acceptance criteria:

- Create new terminal tabs
- Split panes horizontally/vertically
- Reorder tabs
- Rename tabs
- Save terminal profile settings
- Copy/paste works reliably
- Terminal remains responsive under high output

Priority:

- Must have

---

## 6.2 Session Manager

User story:

> As an administrator, I want to organize sessions by project, client, and environment so I can find and launch them quickly.

Acceptance criteria:

- Folder hierarchy
- Tags
- Search
- Favorites
- Session templates
- Last-used sorting
- Session notes

Priority:

- Must have

---

## 6.3 Credential Manager

User story:

> As a security-conscious user, I want my credentials stored safely and released only to the sessions I choose.

Acceptance criteria:

- Windows credential store integration
- Optional encrypted vault
- No plaintext secrets in config files
- Clear warnings when credential access is requested
- SSH key support

Priority:

- Must have

---

## 6.4 SFTP File Browser

User story:

> As a developer, I want to browse and transfer files between my local machine and remote server without leaving the app.

Acceptance criteria:

- Dual-pane local/remote browsing
- Upload/download queue
- Progress indicators
- Resume where possible
- Basic permission editing
- Drag-and-drop support

Priority:

- Must have

---

## 6.5 SSH Tunneling

User story:

> As a developer, I want to create local and dynamic tunnels with a few clicks.

Acceptance criteria:

- Add local forward rules
- Add SOCKS proxy rules
- See tunnel status
- Auto-reconnect tunnels when session reconnects
- Detect port conflicts

Priority:

- Must have for local forwarding; dynamic/remote as should-have

---

## 6.6 RDP Support

User story:

> As a sysadmin, I want to connect to Windows machines through RDP from the same session manager.

Acceptance criteria:

- RDP session type
- Save credentials/profile
- Basic clipboard support
- Stable disconnect/reconnect
- Hardware decoding where available

Priority:

- Must have for v1 GUI release

---

## 6.7 VNC Support

User story:

> As an operator, I want to access legacy VNC endpoints when needed.

Acceptance criteria:

- Basic VNC viewer
- Common encodings
- Save profiles
- Reasonable performance on LAN

Priority:

- Should have

---

## 6.8 X11 Forwarding Compatibility

User story:

> As a Linux user, I want to run legacy X11 applications over SSH when no agent is available.

Acceptance criteria:

- Managed local X server path
- SSH X11 forwarding setup
- Clear diagnostics when X server is unavailable
- xauth handling

Priority:

- Must have as compatibility mode

---

## 6.9 Remote Linux Application Publishing

User story:

> As a developer, I want to launch a Linux GUI application on a remote server and interact with it as if it were a local window.

Acceptance criteria:

- Install/start agent over SSH
- Launch app from catalog or command
- Window appears in client
- Keyboard/mouse input works
- Clipboard works for text where supported
- Session can reconnect after temporary network loss

Priority:

- Must have for strategic v1, but may be alpha/beta staged

---

## 6.10 Plugin Architecture

User story:

> As a power user, I want to extend AetherDesk with custom commands, panels, and automation.

Acceptance criteria:

- Plugin manifest
- Enable/disable plugins
- Sandboxed execution
- Command palette extensions
- Safe APIs

Priority:

- Should have for v1; SDK preview may be staged

---

## 7. User Experience Requirements

### 7.1 Modern UI

- Clean, dense, professional interface
- Dark and light themes
- Command palette
- Keyboard-first operation
- Consistent spacing and typography
- Low visual clutter

### 7.2 Keyboard-First Workflow

- Quick session launcher
- Tab switching shortcuts
- Pane navigation shortcuts
- Command palette
- Terminal search
- Session search

### 7.3 Status Visibility

Users should always see:

- Connection state
- Latency estimate
- Bandwidth mode
- Tunnel status
- Transfer progress
- Agent status for remote apps

---

## 8. Key User Journeys

### 8.1 First SSH Connection

1. User creates new session.
2. Enters host and username.
3. Chooses credential.
4. Connects.
5. Terminal opens.
6. Session saved automatically.

Success:

- Connection established without manual SSH config editing.

---

### 8.2 Daily Multi-Server Workflow

1. User opens app.
2. Previous workspace restored.
3. Tabs reconnect based on policy.
4. User switches between sessions quickly.
5. File transfer initiated from sidebar.

Success:

- App feels like a persistent workspace, not a collection of dialogs.

---

### 8.3 Remote Linux App Session

1. User selects remote app session.
2. Client verifies agent availability.
3. Agent launches app.
4. App window appears locally.
5. User interacts with app.
6. Network blip occurs.
7. Session recovers.

Success:

- User experiences minimal disruption and no data loss.

---

## 9. Release Plan

### 9.1 Release 0.1 — Foundation Preview

Features:

- App shell
- Config store
- Basic SSH terminal
- Basic session list

Exit criteria:

- Stable terminal connections
- Startup target met

---

### 9.2 Release 0.2 — SSH Productivity Beta

Features:

- Multi-tab/split terminal
- SFTP
- Session manager
- Credential manager
- Local tunnels

Exit criteria:

- Can replace basic MobaXterm SSH workflows for internal users

---

### 9.3 Release 0.5 — GUI Compatibility Beta

Features:

- RDP viewer
- VNC viewer
- X11 bridge compatibility mode
- Improved session profiles

Exit criteria:

- Core GUI protocols usable for common scenarios

---

### 9.4 Release 0.8 — Remote App Alpha

Features:

- Linux agent deployment
- Remote app launch
- Basic adaptive streaming
- Clipboard support

Exit criteria:

- Reference apps work with acceptable latency on WAN test profiles

---

### 9.5 Release 1.0 — General Availability

Features:

- Hardened terminal/SFTP/tunneling
- Stable RDP/VNC/X11 compatibility
- Remote app publishing stable for supported scenarios
- Plugin SDK preview
- Auto-update
- Security review completed

Exit criteria:

- Performance, reliability, and security targets met

---

## 10. Product Metrics

| Metric | Target |
|---|---|
| Cold startup time | < 2 seconds |
| Session launch success rate | > 99% for valid configs |
| Crash-free sessions | > 99.5% |
| Terminal keystroke satisfaction | High subjective rating in beta |
| Remote app usability | Comparable or better than X2Go in internal WAN tests |
| Memory usage | Within target budget on reference machine |
| Support tickets due to credential leakage | Zero |
| Plugin sandbox escapes | Zero |

---

## 11. Competitive Positioning

### Against MobaXterm

AetherDesk should win on:

- Lower resource usage
- Faster startup
- Modern UI
- Better remote Linux app streaming path
- Cleaner plugin model

### Against X2Go

AetherDesk should win on:

- Modern architecture
- Better integration with terminal/SFTP/tunnels
- More flexible protocol support
- Potential for better adaptive streaming
- Wayland-aligned future

### Against Raw SSH + External Tools

AetherDesk should win on:

- Unified experience
- Session management
- Credential management
- GUI protocol integration
- Performance tuning

---

## 12. Risks from Product Perspective

| Risk | Impact | Mitigation |
|---|---|---|
| Remote Linux app publishing is delayed | Product differentiation weakened | Ship SSH/SFTP/RDP/VNC value first |
| X11 expectations too high | Support burden | Clearly label X11 as compatibility mode |
| Plugin ecosystem too small | Extensibility goal weak | Provide excellent SDK and examples |
| Performance regressions | Core promise broken | Performance gates in CI |
| Security incident | Severe trust loss | Security review and sandboxing |
| Feature bloat | Startup/resource goals broken | Strict MVP discipline |

---

## 13. Final Recommendation

The optimal path is:

1. Build a fast Rust core and modern Flutter UI.
2. Deliver a best-in-class SSH terminal/SFTP/tunneling MVP first.
3. Add RDP/VNC/X11 compatibility through mature engines.
4. Develop a Linux streaming agent as the strategic differentiator.
5. Implement an adaptive remote presentation protocol inspired by NX/X2Go but designed for modern Wayland/GPU/QUIC realities.
6. Keep X11 as a compatibility mode, not the performance foundation.

This approach gives the best chance of satisfying both business goals:

- A practical MobaXterm alternative in the near term
- A remote Linux GUI experience comparable to or better than X2Go in the medium term




# Addendum A to the AetherDesk Architecture / TRD / PRD

This addendum amends and extends the previously created AetherDesk architecture, Technical Requirements Document, and Product Requirements Document.

It adds detailed requirements and design guidance for the following three mandatory changes:

1. **Sessions must be saveable and deletable through a right-click context menu on the session name.**
2. **All components must be standalone and self-contained because the application must be installable and usable as a standalone product.**
3. **The main screen must behave like a terminal, and any necessary GUI components, such as clock, workspace switcher, dashboards, or remote application surfaces, must use X2Go/NX-style low-latency technology or equivalent rendering principles so the response remains fast.**

---

# 1. Amendment Summary

| # | Change | Affected Documents | Summary |
|---|---|---|---|
| 1 | Session save/delete context menu | PRD, TRD, UX, Architecture | Sessions are persistent objects. Users can right-click a session name to open a context menu with actions such as Open, Edit, Duplicate, Rename, Move, Export, Delete, and Properties. Delete must include confirmation, soft-delete/undo, and safe handling of active sessions. |
| 2 | Standalone deployment | Architecture, TRD, Packaging, Security | AetherDesk must not require external third-party runtimes, external X servers, browsers, Java, Python, .NET, Docker, or separately installed services. All required client components, libraries, assets, plugin runtime, terminal engine, SSH stack, RDP/VNC engines, embedded X server, and agent bootstrap components must be bundled or embedded. |
| 3 | Terminal-first main screen with X2Go-style fast GUI | PRD, TRD, UX, Architecture | The primary application surface must behave like a terminal workspace, not like a dashboard or web app. Graphical widgets and remote GUI surfaces must use a low-latency, damage-based, cached, GPU-composited rendering model aligned with X2Go/NX principles. Remote Linux GUI publishing should prefer X2Go/NX-compatible or NX-style streaming. |

---

# 2. Detailed Addition 1 — Session Save and Delete via Right-Click Context Menu

## 2.1 Product Rule

A session in AetherDesk is a first-class persistent object.

A session may be:

- Created manually
- Created from a template
- Created from a previous connection
- Imported from a file
- Duplicated from another session
- Automatically saved after quick connect if the user chooses to save it

Every saved session must be:

- Persisted locally
- Editable
- Searchable
- Renameable
- Duplicateable
- Exportable
- Deletable

The user must be able to right-click on a session name in the Session Manager and see a context menu containing common session operations.

---

## 2.2 Where the Context Menu Appears

The session context menu must appear when the user right-clicks on any of the following:

| UI Location | Behavior |
|---|---|
| Session name in the session tree | Opens full session context menu |
| Session row in list view | Opens full session context menu |
| Session card in grid view | Opens full session context menu |
| Session tab header | Opens tab/session context menu |
| Favorite session entry | Opens session context menu |
| Search result item | Opens session context menu |

The context menu must also be accessible through keyboard actions:

| Action | Shortcut |
|---|---|
| Open context menu for selected session | `Shift + F10` or `Context Menu` key |
| Delete selected session | `Delete` |
| Rename selected session | `F2` |
| Duplicate selected session | `Ctrl + D` or product-defined shortcut |
| Open session | `Enter` |
| Open session in new tab | `Ctrl + Enter` |

If multiple sessions are selected, the context menu must support bulk operations where appropriate.

---

## 2.3 Session Context Menu Items

The context menu displayed on a session name must include the following options.

### 2.3.1 Basic Session Actions

| Menu Item | Behavior | Enabled When |
|---|---|---|
| Open / Connect | Opens the session using its default action | Session is not already active, or user wants another instance |
| Open in New Tab | Opens session in a new tab | Always, unless session type disallows multiple instances |
| Open in Split Pane | Opens session in a horizontal or vertical split | Always, unless session type disallows split display |
| Reconnect | Reconnects an existing session | Session is disconnected or reconnectable |
| Disconnect | Disconnects an active session | Session is active |
| Edit... | Opens session editor dialog/page | Always |
| Rename | Starts inline rename | Always |
| Duplicate | Creates a copy of the session | Always |
| Delete... | Opens delete confirmation | Always |
| Properties | Shows metadata and diagnostics | Always |

---

### 2.3.2 Organization Actions

| Menu Item | Behavior |
|---|---|
| Add to Favorites | Marks session as favorite |
| Remove from Favorites | Removes favorite mark |
| Move To | Opens folder selection submenu or dialog |
| Copy to Folder | Copies session into another folder |
| Add Tag | Opens tag editor |
| Clear Tags | Removes tags |
| Pin to Top | Pins session to top of session list |
| Unpin | Removes pinned state |

---

### 2.3.3 Import / Export Actions

| Menu Item | Behavior |
|---|---|
| Export... | Exports selected session or selected sessions to file |
| Export as Template... | Saves session as reusable template |
| Copy Session URI | Copies internal session URI to clipboard |
| Copy Connection String | Copies user-safe connection string, excluding secrets |

Export must never include plaintext secrets unless the user explicitly chooses a secure export and confirms the risk.

---

### 2.3.4 Diagnostics and Advanced Actions

| Menu Item | Behavior |
|---|---|
| View Logs | Opens logs filtered to this session |
| View Metrics | Opens latency, bandwidth, and connection stats |
| Reset Layout | Resets saved layout for this session |
| Clear Cached Session State | Clears cached reconnect state |
| Forget Known Host Key | Removes cached host trust decision for this session |
| Repair Session | Validates and repairs broken session configuration |

---

## 2.4 Example Context Menu

For a normal saved SSH session named `prod-web-01`, the context menu should look like this:

```text
Open
Open in New Tab
Open in Split Pane
--------------------------------
Reconnect
Disconnect
--------------------------------
Edit...
Rename
Duplicate...
Delete...
--------------------------------
Add to Favorites
Move To                >
Add Tag                >
--------------------------------
Export...
Copy Session URI
--------------------------------
Properties
```

For a folder named `Production`, the context menu should include folder-specific operations:

```text
Open All
Connect All
--------------------------------
New Session Here
New Folder
Rename
Delete Folder...
--------------------------------
Move To                >
Export Folder...
```

For multiple selected sessions, the context menu should include bulk operations:

```text
Open All
Connect All
--------------------------------
Move To                >
Add Tag                >
Export Selected...
Delete Selected...
```

---

## 2.5 Session Save Behavior

### 2.5.1 Automatic Saving

Sessions must be automatically saved in the following cases:

| Event | Save Behavior |
|---|---|
| User creates a session and presses Save | Session is persisted immediately |
| User edits an existing session and presses Save | Changes are persisted immediately |
| User connects from a saved session | Last-used timestamp is updated |
| User changes layout and auto-save layout is enabled | Layout state is saved |
| User renames a session | Name change is saved immediately |
| User moves a session to another folder | Move operation is saved immediately |
| User deletes a session | Deletion state is saved immediately |

The application must not lose session definitions if the app crashes or the machine restarts.

---

### 2.5.2 Explicit Save

The session editor must provide explicit actions:

| Button | Behavior |
|---|---|
| Save | Saves changes and closes editor |
| Save & Connect | Saves changes and immediately connects |
| Save As Template | Saves the session as a reusable template |
| Cancel | Discards unsaved changes after confirmation if dirty |
| Apply | Saves changes without closing editor |

If the session has unsaved changes, the editor must display a dirty indicator such as:

```text
prod-web-01 •
```

or:

```text
Unsaved changes
```

---

### 2.5.3 Quick Connect Sessions

When a user connects through Quick Connect without selecting a saved session, the connection may be treated as ephemeral by default.

After successful connection, the UI should offer:

```text
Save this session?
```

With actions:

- Save
- Save As...
- Don’t Save

If the user chooses Save, the session becomes a normal persistent session and must support the same right-click context menu.

---

## 2.6 Session Delete Behavior

### 2.6.1 Delete Entry Point

The Delete action must be available from:

- Session context menu
- Session editor
- Keyboard `Delete` action
- Bulk selection context menu

The primary entry point requested is:

> Right-click on the session name → Delete...

---

### 2.6.2 Delete Confirmation

Deleting a session must require confirmation unless the user has enabled an expert mode with undo-only deletion.

Default confirmation dialog:

```text
Delete session?

You are about to delete:

    prod-web-01

This will remove the saved session definition.
Saved credentials associated only with this session may also be marked for deletion.

[ ] Also delete associated credentials
[ ] Also delete saved layout and cache

Cancel        Delete
```

The confirmation dialog must include:

- Session name
- Session type
- Warning if the session is currently active
- Optional checkbox for deleting associated credentials
- Optional checkbox for deleting cached state
- Cancel action
- Delete action

---

### 2.6.3 Soft Delete and Undo

Deleted sessions should first enter a soft-deleted state.

Recommended behavior:

| Setting | Default |
|---|---|
| Soft-delete retention period | 30 days |
| Show undo notification after delete | Yes |
| Allow restore from Recycle/Deleted area | Yes |
| Permanent delete requires explicit confirmation | Yes |

After deletion, show a transient notification:

```text
Session deleted: prod-web-01
Undo
```

If the user clicks Undo, the session is restored with its previous folder, tags, name, and metadata.

---

### 2.6.4 Permanent Delete

The user may permanently delete a session from:

- Deleted Sessions view
- Session context menu if soft delete is disabled
- Delete confirmation dialog with “Permanently delete” selected

Permanent delete dialog:

```text
Permanently delete session?

This action cannot be undone.

Session:
    prod-web-01

Cancel        Permanently Delete
```

---

### 2.6.5 Deleting an Active Session

If the session is currently connected:

```text
This session is currently active.

Deleting the session will disconnect it and remove the saved session definition.

Cancel        Disconnect and Delete
```

If the user confirms:

1. The session connection is gracefully disconnected.
2. The session definition is moved to soft-deleted state.
3. Any associated UI tab may remain open as an untitled disconnected session, or close depending on user preference.

Default recommendation:

- If the tab is active and the session is deleted, close the tab after confirmation.
- If the user has unsaved terminal output or active transfer, warn before closing.

---

### 2.6.6 Deleting a Folder

Deleting a folder must handle children explicitly.

Confirmation:

```text
Delete folder?

Folder:
    Production

This folder contains:
    12 sessions
    3 subfolders

Choose one:
    Delete folder only and move children to parent
    Delete folder and all contents

Cancel        Delete
```

If deleting all contents, the operation must support undo for the whole tree when soft delete is enabled.

---

## 2.7 Session Persistence Specification

### 2.7.1 Storage Location

Standalone storage locations:

#### Installed mode

```text
%APPDATA%\AetherDesk\
```

Recommended files:

```text
%APPDATA%\AetherDesk\sessions.db
%APPDATA%\AetherDesk\settings.toml
%APPDATA%\AetherDesk\credentials.vault
%APPDATA%\AetherDesk\known_hosts.toml
%APPDATA%\AetherDesk\layouts\
%APPDATA%\AetherDesk\cache\
%APPDATA%\AetherDesk\logs\
```

#### Portable mode

```text
<AetherDesk executable directory>\data\
```

Example:

```text
AetherDesk\
    aetherdesk.exe
    data\
        sessions.db
        settings.toml
        credentials.vault
        known_hosts.toml
        layouts\
        cache\
        logs\
```

Portable mode must not require administrator privileges.

---

### 2.7.2 Storage Format

Recommended implementation:

| Data Type | Storage Format |
|---|---|
| Sessions | SQLite database |
| Settings | TOML |
| Known hosts | TOML or SQLite |
| Layouts | JSON or SQLite |
| Credentials | OS credential store or encrypted local vault |
| Deleted sessions | SQLite table with soft-delete metadata |

SQLite must be bundled with the application. It must not be an external dependency.

---

### 2.7.3 Session Record Fields

Each session record must include at least:

```text
id
name
type
folder_id
tags
is_favorite
is_pinned
is_deleted
deleted_at
created_at
updated_at
last_used_at
host
port
username
auth_method_ref
proxy_jump_ref
environment_variables
startup_directory
startup_commands
terminal_profile_id
display_profile_id
tunnels
remote_app_settings
rdp_settings
vnc_settings
x11_settings
layout_state_ref
notes
```

For remote Linux application sessions:

```text
agent_mode
app_command
app_environment
app_window_mode
clipboard_policy
audio_policy
file_bridge_policy
fallback_mode
```

---

### 2.7.4 Atomic Writes

Session persistence must be atomic.

Requirements:

- Use SQLite transactions
- Do not corrupt database if app crashes during save
- Maintain backup file or WAL journal
- Validate schema on startup
- Repair minor corruption where possible
- Preserve previous version backup during upgrades

---

## 2.8 Session Context Menu API

The core must expose APIs to the UI.

Example operations:

```rust
save_session(session: SessionDraft) -> Session
delete_session(session_id: SessionId, options: DeleteOptions) -> DeleteResult
restore_session(session_id: SessionId) -> Session
duplicate_session(session_id: SessionId) -> Session
move_session(session_id: SessionId, folder_id: FolderId) -> Session
rename_session(session_id: SessionId, name: String) -> Session
export_session(session_ids: Vec<SessionId>, format: ExportFormat) -> ExportResult
```

Example events:

```text
SessionCreated
SessionUpdated
SessionDeleted
SessionRestored
SessionMoved
SessionDuplicated
SessionFavoriteChanged
SessionBulkDeleted
```

---

## 2.9 Edge Cases

| Edge Case | Required Behavior |
|---|---|
| Session name is empty | Use default name such as “Untitled Session” |
| Duplicate session name | Allowed, but UI should show distinguishing ID or folder path |
| Very long session name | Truncate visually with tooltip, preserve full name in storage |
| Unicode session names | Fully supported |
| Session storage is read-only | Show clear warning; allow export but not save |
| Session database corrupted | Attempt recovery and show recovery dialog |
| Multiple app instances open | Use file locking or portable single-instance policy |
| Delete session while transfer is active | Warn and ask whether to cancel transfer |
| Delete session while remote app is running | Warn that remote app may be terminated or detached depending on policy |
| Context menu on search result | Actions apply to original session object |
| Multi-select with mixed session types | Bulk actions limited to common supported operations |

---

## 2.10 Updated PRD User Stories for Session Save/Delete

### Story 1 — Save Session

> As a user, I want to save a session so I can reconnect to it later without re-entering connection details.

Acceptance criteria:

- User can create a session and save it.
- Saved session appears in the session manager.
- Saved session persists after app restart.
- Session can be opened from the session list.

---

### Story 2 — Right-Click Session

> As a user, I want to right-click on a session name to manage it quickly.

Acceptance criteria:

- Right-clicking a session name opens a context menu.
- Menu includes Open, Edit, Rename, Duplicate, Delete, and Properties.
- Menu is accessible by keyboard.
- Actions operate on the correct session.

---

### Story 3 — Delete Session

> As a user, I want to delete sessions I no longer need.

Acceptance criteria:

- Delete is available from the session context menu.
- User is asked to confirm deletion.
- Deleted session can be undone shortly after deletion.
- Permanently deleting requires explicit confirmation.
- Active session deletion warns the user.

---

### Story 4 — Bulk Delete

> As a power user, I want to delete multiple sessions at once.

Acceptance criteria:

- User can multi-select sessions.
- Context menu shows Delete Selected.
- Confirmation dialog shows number of selected sessions.
- Undo restores all selected sessions if soft delete is enabled.

---

## 2.11 New TRD Requirements for Session Context Menu

| ID | Requirement | Priority |
|---|---|---|
| TR-SESS-CTX-001 | The UI must provide a context menu when right-clicking a session name | Must |
| TR-SESS-CTX-002 | The context menu must include Open, Edit, Rename, Duplicate, and Delete | Must |
| TR-SESS-CTX-003 | Sessions must persist across application restarts | Must |
| TR-SESS-CTX-004 | Session deletion must require confirmation by default | Must |
| TR-SESS-CTX-005 | Deleted sessions must support soft delete and undo by default | Should |
| TR-SESS-CTX-006 | Deleting an active session must warn the user | Must |
| TR-SESS-CTX-007 | Session storage must be local and standalone | Must |
| TR-SESS-CTX-008 | Session export must not include plaintext secrets unless explicitly requested | Must |
| TR-SESS-CTX-009 | Bulk session operations must be supported for multi-select | Should |
| TR-SESS-CTX-010 | Session operations must emit events for UI and plugin systems | Should |

---

# 3. Detailed Addition 2 — Standalone Components and Standalone Installation

## 3.1 Standalone Definition

For AetherDesk, “standalone” means:

> The application must be installable and runnable on a supported operating system without requiring the user to install any external runtime, framework, service, interpreter, browser, X server, database server, package manager component, or third-party helper.

AetherDesk must be self-contained.

The product may use operating-system-provided capabilities such as:

- Windows kernel APIs
- Windows GUI/windowing system
- Windows networking stack
- Windows secure credential storage
- Linux kernel interfaces for remote agent, such as `/dev/uinput` or `/dev/dri`

However, it must not require user-installed third-party dependencies.

---

## 3.2 Standalone Rule

Every required component must be one of the following:

1. Compiled directly into the application binary
2. Bundled as a private library inside the installation directory
3. Bundled as a private helper executable inside the installation directory
4. Downloaded and cached automatically by the application in a self-contained way if explicitly allowed by policy
5. Not required at all

The application must not require:

- External .NET runtime
- External Java runtime
- External Python interpreter
- External Node.js runtime
- External browser or WebView runtime if avoidable
- External X server such as VcXsrv, Xming, X410, or Cygwin/X
- External SSH client binary unless optional and not required
- External RDP client
- External VNC client
- External database server
- External message broker
- External container runtime
- External package manager
- Internet access for first launch, except for remote connections or optional updates

---

## 3.3 Client Standalone Component Matrix

| Component | Standalone Requirement | Implementation Direction |
|---|---|---|
| Application shell | Must be bundled | Native launcher plus bundled UI runtime |
| UI runtime | Must be bundled | Flutter engine/runtime bundled in install directory |
| Core engine | Must be bundled | Rust core compiled as native library/binary |
| Terminal engine | Must be bundled | Implemented in Rust/core, no external terminal emulator required |
| SSH client | Must be bundled | Built-in Rust SSH stack or bundled OpenSSH-compatible component |
| SFTP client | Must be bundled | Built into SSH stack |
| Tunnel manager | Must be bundled | Built into SSH stack |
| SOCKS proxy | Must be bundled | Built into tunnel manager |
| Credential vault | Must be bundled | OS credential store plus optional built-in encrypted vault |
| Session database | Must be bundled | SQLite bundled or embedded |
| RDP engine | Must be bundled | FreeRDP or equivalent bundled as private library |
| VNC engine | Must be bundled | Built-in or bundled RFB client |
| X11 server | Must be bundled for X11 support | Embedded X server component included in installer |
| X11 forwarding adapter | Must be bundled | Built into SSH/X bridge |
| Remote GUI streaming engine | Must be bundled | Built-in NX/X2Go-style or X2Go-compatible component |
| Codec libraries | Must be bundled | Bundled private codec libraries |
| Plugin runtime | Must be bundled | Embedded WASM runtime and/or isolated native plugin broker |
| ICU / Unicode data | Must be bundled | Bundled with UI/text engine |
| Fonts | Must be bundled | Bundled default terminal and UI fonts |
| Root certificates | Must be bundled or use OS trust store | Prefer OS trust store, but bundled fallback allowed |
| Updater | Optional | If included, must be part of installation and not external installer |
| Diagnostic tools | Must be bundled if required | Built into app or bundled helper |

---

## 3.4 Windows Standalone Installation Requirements

### 3.4.1 Installer

The Windows installer must produce a complete standalone installation.

Supported formats:

- MSIX
- MSI
- Portable ZIP or folder

The installer must include:

- Main executable
- All required DLLs
- Flutter/UI runtime assets
- Rust core libraries
- SSH/SFTP/tunnel components
- RDP/VNC libraries
- Embedded X server component
- Codec libraries
- Plugin runtime
- Default themes
- Default fonts
- Default terminal profiles
- Session templates
- Agent bootstrap binaries if shipped with client
- License files
- Third-party notices

---

### 3.4.2 No External Runtime Dependencies

The Windows build must not require installation of:

- Visual C++ Redistributable, unless statically linked or privately bundled
- .NET Desktop Runtime
- Windows App SDK not already present, unless bundled privately
- Python
- Node.js
- Java
- WSL
- Cygwin
- MSYS2
- VcXsrv
- Xming
- X410
- PuTTY
- OpenSSH client
- WinPCap/Npcap unless explicitly required by an optional feature and bundled with consent

If a required dependency cannot be statically linked, it must be installed privately inside the application directory.

---

### 3.4.3 Portable Mode

AetherDesk must support portable standalone mode.

Portable mode requirements:

- Run from a folder without installation
- Store configuration in local `data/` directory
- Not require administrator rights
- Not require registry writes
- Not require global services
- Support all core features except those blocked by OS policy
- Allow read-only executable directory with writable data directory

Example portable layout:

```text
AetherDesk/
    aetherdesk.exe
    aetherdesk_core.dll
    flutter_engine.dll
    icudtl.dat
    assets/
    plugins/
    agents/
    xserver/
    codecs/
    fonts/
    licenses/
    data/
        sessions.db
        settings.toml
        known_hosts.toml
        credentials.vault
        logs/
        cache/
        layouts/
```

---

## 3.5 Embedded X Server Requirement

Because AetherDesk must support X11 forwarding while remaining standalone, it must include an embedded X server component.

### 3.5.1 Embedded X Server Component

Component name suggestion:

```text
AetherDesk Embedded X Server
```

Executable/module:

```text
aetherdesk-xserver.exe
```

or embedded as a private component:

```text
xserver/ad-xserver.exe
```

### 3.5.2 Functional Requirements

The embedded X server must:

- Run locally on Windows
- Start automatically when an X11 session requires it
- Provide a local display, for example `:0`
- Support xauth authentication
- Support MIT-MAGIC-COOKIE-1
- Support core X11 protocol
- Support common extensions required by GUI applications
- Support basic RENDER extension
- Support basic Composite extension
- Support SHAPE extension
- Support XKB keyboard handling
- Support XTEST where needed for input compatibility
- Render to a local AetherDesk surface or window
- Allow copy/paste integration where possible
- Shut down cleanly when no X11 sessions require it
- Run as a normal user process, not as a service
- Not require installation outside the AetherDesk directory

---

### 3.5.3 Embedded X Server Extensions

Minimum recommended X extension support:

| Extension | Required |
|---|---:|
| Core X11 | Must |
| MIT-MAGIC-COOKIE-1 | Must |
| RENDER | Must |
| Composite | Should |
| SHAPE | Should |
| XKB | Must |
| XTEST | Should |
| RANDR | Should |
| DAMAGE | Should |
| GLX | Optional |
| MIT-SHM | Optional, only where safe/local |

OpenGL support may be optional because it increases complexity.

---

### 3.5.4 Embedded X Server Security

Requirements:

- Listen only on local interfaces or local named pipes/sockets where possible
- Require xauth cookie
- Reject unauthorized local connections
- Do not expose display to network unless user explicitly enables it
- Do not run as administrator
- Isolate X server process from main UI process where practical
- Sanitize logs to avoid leaking cookies

---

## 3.6 Remote Agent Standalone Requirements

For remote Linux application publishing, the remote agent should also be standalone as far as the target operating system allows.

### 3.6.1 Agent Deployment Model

The client should be able to deploy the agent over SSH/SFTP without requiring root or package installation.

Recommended remote agent path:

```text
~/.aetherdesk/agent/<version>/
```

Example:

```text
~/.aetherdesk/agent/1.0.0/
    ad-agent
    libs/
    share/
    bin/
    runtime/
    manifest.json
```

### 3.6.2 Agent Standalone Goals

The agent should:

- Be uploaded by the client over SSH/SFTP
- Run from the user’s home directory
- Not require root
- Not require system package installation
- Not require external X server installation if the agent provides its own display/compositor path
- Include required libraries where possible
- Detect missing OS interfaces and degrade gracefully
- Support software encoding fallback if GPU encoding is unavailable

---

### 3.6.3 Remote Agent Limitations

Some items cannot be fully bundled because they are provided by the remote Linux kernel or host environment:

- Kernel interfaces such as `/dev/uinput`
- DRM/GPU device nodes such as `/dev/dri/*`
- System Wayland compositor services, if integrating with existing desktop
- Audio server sockets, if audio integration is enabled
- System clipboard managers, if present

Standalone in this context means:

> No package installation, no root installation, and no mandatory external helper, but OS-provided kernel/device interfaces may be used.

---

### 3.6.4 Agent Components to Bundle

| Agent Component | Standalone Handling |
|---|---|
| Agent supervisor | Static or mostly static Rust binary |
| Session manager | Bundled with agent |
| Wayland compositor/host | Bundled or built into agent |
| XWayland | Bundled where possible, or host-provided fallback |
| Input injection | Uses OS interfaces; helper bundled if needed |
| Clipboard bridge | Bundled |
| Encoder | Bundled software fallback; GPU APIs used if available |
| Transport adapter | Bundled |
| Manifest/version metadata | Bundled |

---

## 3.7 Standalone GUI Technology Constraint

The application must not depend on a browser or web runtime for core UI behavior.

Therefore:

- The main terminal surface must not be implemented with HTML/CSS/JavaScript inside an external browser.
- Core GUI widgets must not require an external web runtime.
- Remote GUI surfaces must not require an external VNC/RDP/X11 helper installed by the user.
- Performance-critical surfaces should use native GPU composition, terminal-style rendering, or X2Go/NX-style streaming.

---

## 3.8 Standalone Update and Repair

Even though the application is standalone, it should support:

- Self-update if the user enables it
- Offline update from file
- Repair installation
- Component integrity verification
- Rollback if an update fails

Update package must include all required components. It must not download mandatory missing runtimes at first launch.

---

## 3.9 New TRD Requirements for Standalone Deployment

| ID | Requirement | Priority |
|---|---|---|
| TR-DEP-STD-001 | The application must be installable and runnable as a standalone product | Must |
| TR-DEP-STD-002 | The application must not require external third-party runtimes | Must |
| TR-DEP-STD-003 | All required libraries must be bundled or embedded | Must |
| TR-DEP-STD-004 | All required helper executables must be bundled inside the install directory | Must |
| TR-DEP-STD-005 | Portable mode must store data locally and not require admin rights | Should |
| TR-DEP-STD-006 | X11 forwarding support must include an embedded X server component | Must |
| TR-DEP-STD-007 | The embedded X server must not require external X server installation | Must |
| TR-DEP-STD-008 | Remote agent deployment should not require root or package installation | Should |
| TR-DEP-STD-009 | Core features must work without internet access except for remote connections | Must |
| TR-DEP-STD-010 | Installer must include fonts, assets, codecs, runtime libraries, and licenses | Must |
| TR-DEP-STD-011 | The application must not require WSL for core functionality | Must |
| TR-DEP-STD-012 | The application must not require Docker for core functionality | Must |
| TR-DEP-STD-013 | Updates must be fully bundled or explicitly user-initiated | Should |

---

# 4. Detailed Addition 3 — Terminal-First Main Screen and X2Go-Style Fast GUI

## 4.1 Product Rule

The main AetherDesk screen must behave primarily like a terminal workspace.

This means:

- The central surface is a terminal emulator.
- Keyboard-first operation is the default.
- Session launch, command entry, tab management, and pane control should feel like a terminal-centric application.
- GUI components must not dominate the main screen unless the user opens a remote GUI session.
- GUI elements must not introduce heavy startup or rendering overhead.
- Remote GUI sessions must appear embedded in the terminal workspace, not as disconnected external tools.

AetherDesk should feel like a fast terminal that can also display remote GUI surfaces, rather than a dashboard that contains a terminal.

---

## 4.2 Terminal-First Main Screen Behavior

### 4.2.1 Startup Behavior

When AetherDesk starts, the main screen should show one of the following:

#### Preferred default

A ready terminal session:

```text
AetherDesk Terminal

Press Ctrl+K to quick connect, Ctrl+T for a new terminal, or Ctrl+P for the command palette.
```

#### Alternative default

A minimal terminal-based session launcher:

```text
aetherdesk>
```

This launcher should support commands such as:

```text
connect prod-web-01
open session prod-web-01
new terminal
list sessions
delete session prod-web-01
save session
export sessions
help
```

The launcher must not replace the terminal. It should behave like an internal command line inside the terminal-first environment.

---

### 4.2.2 Main Layout

Recommended default layout:

```text
+----------------------------------------------------------------------------------+
| Menu / Command Bar / Tab Bar                                                     |
+----------------+-----------------------------------------------------------------+
|                |                                                                 |
| Session        |                                                                 |
| Manager        |                 Main Terminal Canvas                            |
|                |                                                                 |
| - Favorites    |                                                                 |
| - Folders      |                                                                 |
| - Search       |                                                                 |
|                |                                                                 |
+----------------+-----------------------------------------------------------------+
| Status Line: session | host | latency | bandwidth | mode | clock                |
+----------------------------------------------------------------------------------+
```

The main canvas must default to terminal rendering.

---

### 4.2.3 Terminal Canvas Rules

The main terminal canvas must:

- Accept keyboard focus immediately after startup
- Support multiple tabs
- Support split panes
- Support fast scrolling
- Support copy/paste
- Support terminal applications such as vim, nano, htop, tmux, ssh, less, and bash
- Maintain low input latency
- Avoid blocking during network operations
- Render status overlays without disrupting terminal content

---

## 4.3 GUI Components Inside a Terminal-First Application

Although the main screen is terminal-first, some GUI components are still necessary.

Examples:

- Clock
- Session status indicator
- Latency indicator
- Workspace switcher / virtual desktop switcher
- Remote application launcher
- Remote application window surface
- File transfer progress panel
- Notification toasts
- Command palette
- Session manager tree

These components must be lightweight and fast.

---

## 4.4 Fast GUI Plane

AetherDesk must define a **Fast GUI Plane**.

The Fast GUI Plane is the rendering layer used for GUI components that must remain highly responsive.

It must use one of the following implementation strategies:

1. **Native GPU immediate-mode rendering**
2. **Terminal renderer shared pipeline**
3. **X2Go/NX-style damage-based tile rendering**
4. **Remote X2Go/NX-compatible surface streaming**

The Fast GUI Plane must not rely on:

- External browser rendering
- Heavy DOM-style layout engines
- JavaScript-heavy UI frameworks for core interaction surfaces
- Separate heavyweight GUI processes unless required for isolation and performance remains acceptable

---

## 4.5 X2Go/NX-Style Technology Requirement

The user requirement states:

> Necessary GUI, like clock or virtuoso, must use the same technology as X2Go so the response will be fast.

Interpretation:

AetherDesk should use X2Go/NX-style technology or an equivalent NX-compatible streaming/caching model for graphical components that require fast interactive response, especially remote GUI components.

This applies especially to:

- Remote Linux application windows
- Remote desktop surfaces
- Remote workspace switchers
- Remote graphical dashboards
- Remote GUI launchers
- Remote widgets that are hosted on the remote Linux host

For local widgets such as clock or local status indicators, the implementation may use the same rendering principles as X2Go/NX:

- Damage tracking
- Region updates
- Caching
- GPU composition
- Low-latency input handling
- Minimal redraws

---

## 4.6 X2Go/NX-Style GUI Engine

The architecture should include a dedicated engine:

### Component Name

```text
AetherDesk NX-Compatible Display Engine
```

Alternative name:

```text
ANX Engine
```

### Purpose

Provide X2Go-like responsiveness for remote GUI surfaces.

### Required Characteristics

| Characteristic | Requirement |
|---|---|
| Transport | SSH-based bootstrap, with optional secure direct media path |
| Remote agent | Required for high-performance Linux GUI publishing |
| Caching | Glyph, tile, pixmap, image, and window-state caching |
| Compression | Lossless for text/UI, lossy for images/video where appropriate |
| Region updates | Damage-based updates only |
| Input priority | Input messages must not be blocked by frame updates |
| Session persistence | Resume/reconnect support |
| Window model | Support seamless application windows where possible |
| Clipboard | Text clipboard synchronization |
| Rendering | GPU-composited where available |
| Fallback | X11 compatibility mode when agent or NX mode is unavailable |

---

## 4.7 Main Screen Modes

The main screen should support the following modes.

### 4.7.1 Terminal Mode

Default mode.

The main canvas displays:

- Local terminal
- SSH terminal
- Serial/console terminal if supported later
- Internal command line

This mode must be fastest.

---

### 4.7.2 Remote GUI Mode

The main canvas displays a remote GUI session:

- Remote Linux application window
- Remote desktop
- X11 forwarded application
- RDP session
- VNC session

Remote GUI sessions should appear as tabs or panes inside the main terminal workspace.

---

### 4.7.3 Hybrid Mode

The main canvas contains both:

- Terminal pane
- Remote GUI pane

Example:

```text
+--------------------------------------+--------------------------------------+
|                                      |                                      |
| SSH Terminal                         | Remote Linux Application             |
|                                      |                                      |
|                                      |                                      |
+--------------------------------------+--------------------------------------+
```

Hybrid mode is essential for developer workflows.

---

## 4.8 Clock, Workspace Switcher, and Similar Widgets

### 4.8.1 Local Clock

The clock should be implemented as a lightweight local status-line widget.

It must not cause full-screen redraws.

Recommended implementation:

- Rendered in status line
- Damage-only update once per second or per minute depending on format
- Uses same GPU text pipeline as terminal text
- Does not require remote session
- Does not require web technologies

Status line example:

```text
prod-web-01 | SSH | RTT 128 ms | 3.2 Mbps | NX-GUI: ON | 14:32:07
```

---

### 4.8.2 Local Workspace / Virtual Desktop Switcher

If the workspace switcher is local, it should be rendered as a lightweight HUD overlay or terminal-style panel.

It must:

- Open instantly
- Use cached icons or text labels
- Not require heavy animations
- Support keyboard switching
- Preserve focus behavior

Example terminal-style representation:

```text
Workspaces:
  1: Ops
  2: Dev
  3: DB
  4: Remote Apps
```

If the workspace switcher is remote-hosted, it must be delivered through the NX/X2Go-style GUI engine.

---

### 4.8.3 Remote Widgets

If a widget is hosted on the remote Linux side, such as:

- Remote clock
- Remote dashboard
- Remote virtual desktop manager
- Remote application launcher
- Remote GUI control panel

then it must be displayed through the X2Go/NX-style GUI plane.

This ensures that the widget uses the same fast remote display technology as remote applications.

---

## 4.9 GUI Responsiveness Requirements

| Requirement | Target |
|---|---|
| Main screen terminal focus after startup | Immediate, under 1 second on reference hardware |
| Context menu open time | Under 80 ms |
| Command palette open time | Under 100 ms |
| Clock/status update overhead | Negligible, no full redraw |
| Remote GUI input added latency | Target under 50 ms excluding network RTT |
| Remote GUI frame update | Damage-based, not full-frame unless necessary |
| Widget animation | Optional and must not block input |
| Terminal scrolling | Smooth and non-blocking |
| GUI overlay rendering | Must not corrupt terminal scrollback or input focus |

---

## 4.10 Terminal-First Interaction Model

### 4.10.1 Keyboard Shortcuts

Recommended default shortcuts:

| Shortcut | Action |
|---|---|
| `Ctrl + T` | New terminal tab |
| `Ctrl + W` | Close current tab/pane |
| `Ctrl + Shift + T` | Reopen last closed session/tab |
| `Ctrl + K` | Quick connect/command bar |
| `Ctrl + P` | Command palette |
| `Ctrl + B` | Toggle session manager sidebar |
| `Ctrl + Shift + O` | Open session search |
| `Ctrl + Tab` | Next tab |
| `Ctrl + Shift + Tab` | Previous tab |
| `Alt + Left/Right/Up/Down` | Move between panes |
| `Ctrl + Shift + Enter` | Toggle focused pane zoom |
| `Ctrl + ,` | Settings |
| `Ctrl + Shift + F` | Terminal search |

---

### 4.10.2 Internal Terminal Commands

The terminal-first launcher may support internal commands prefixed by a shell-safe token.

Example:

```text
ad> list sessions
ad> connect prod-web-01
ad> new terminal
ad> split right
ad> split down
ad> session save
ad> session delete prod-web-01
ad> export sessions --format json
```

These commands must be optional. They should complement, not replace, the graphical session manager.

---

## 4.11 Updated PRD User Stories for Terminal-First GUI

### Story 1 — Terminal-First Startup

> As a user, I want AetherDesk to open directly into a terminal-style workspace so I can start working immediately.

Acceptance criteria:

- App opens with terminal canvas or terminal launcher.
- Keyboard focus is available immediately.
- No heavyweight dashboard blocks startup.
- User can open a session with keyboard only.

---

### Story 2 — Embedded Remote GUI

> As a user, I want remote GUI applications to open inside the main workspace instead of requiring separate tools.

Acceptance criteria:

- Remote app opens in a tab or pane.
- Terminal and remote GUI can be used side by side.
- Input focus switches predictably.
- Remote GUI uses low-latency streaming.

---

### Story 3 — Fast Widgets

> As a user, I want status widgets such as clock and connection indicators to update without slowing the terminal.

Acceptance criteria:

- Clock updates do not cause terminal redraw storms.
- Status indicators update efficiently.
- UI remains responsive during high terminal output.
- Widgets do not increase startup time significantly.

---

### Story 4 — X2Go-Style Remote GUI Responsiveness

> As a user, I want remote Linux GUI applications to respond quickly over WAN, similar to or better than X2Go.

Acceptance criteria:

- Remote GUI uses NX/X2Go-style caching/compression or equivalent.
- Input is prioritized over bulk display traffic.
- Sessions can recover from temporary disconnections.
- Text remains sharp while images/video are adaptively compressed.

---

## 4.12 New TRD Requirements for Terminal-First GUI

| ID | Requirement | Priority |
|---|---|---|
| TR-UI-TERM-001 | The main screen must default to a terminal-first workspace | Must |
| TR-UI-TERM-002 | The terminal canvas must receive keyboard focus immediately after startup | Must |
| TR-UI-TERM-003 | Remote GUI sessions must be embeddable as tabs or panes | Must |
| TR-UI-TERM-004 | Local widgets must use damage-based rendering or equivalent low-overhead rendering | Must |
| TR-UI-TERM-005 | The UI must not require an external browser or web runtime for core features | Must |
| TR-UI-TERM-006 | Remote Linux GUI surfaces must use X2Go/NX-style rendering or an equivalent adaptive cached streaming model | Must |
| TR-UI-TERM-007 | Input events for GUI surfaces must be prioritized over large display updates | Must |
| TR-UI-TERM-008 | Clock/status widgets must not cause full-screen redraws | Must |
| TR-UI-TERM-009 | The main screen must support hybrid terminal and remote GUI panes | Must |
| TR-UI-TERM-010 | Terminal-first keyboard shortcuts must be available | Should |
| TR-UI-TERM-011 | An internal command-line launcher may be provided inside the terminal workspace | Could |
| TR-UI-TERM-012 | GUI overlays must not block terminal input unless explicitly modal | Must |

---

# 5. Architecture Amendments

The original architecture should be updated with the following components.

---

## 5.1 New or Updated Components

| Component | Purpose |
|---|---|
| Session Context Menu Service | Provides session actions, bulk actions, delete/undo, export, and move operations |
| Session Persistence Store | SQLite-based local standalone storage for sessions, folders, tags, and deleted items |
| Embedded X Server | Bundled local X server for standalone X11 forwarding support |
| NX-Compatible Display Engine | X2Go/NX-style remote GUI streaming engine |
| Fast GUI Plane | Low-latency local/remote GUI composition layer |
| Standalone Packaging Manager | Ensures all dependencies are bundled and versioned |
| Agent Bootstrap Service | Deploys standalone remote agent over SSH/SFTP |
| Portable Storage Manager | Handles installed versus portable data locations |
| Integrity Checker | Verifies bundled components, plugins, and agent binaries |

---

## 5.2 Updated High-Level Diagram

```text
+----------------------------------------------------------------------------------+
|                               AetherDesk Client                                  |
|                                                                                  |
|  +----------------------+   +----------------------+   +----------------------+ |
|  | Terminal-First UI    |   | Session Manager      |   | Context Menu Service | |
|  | Main terminal canvas |   | Save/Delete/Move     |   | Right-click actions  | |
|  +----------+-----------+   +----------+-----------+   +----------+-----------+ |
|             |                          |                          |             |
|             +-------------+------------+--------------------------+             |
|                           |                                                      |
|                +----------v-----------+                                          |
|                | Core Broker          |                                          |
|                +----+-----+-----+-----+                                          |
|                     |     |     |                                                |
|       +-------------+     |     +----------------+                               |
|       |                   |                      |                               |
| +-----v------+    +-------v--------+     +-------v--------+                     |
| | Terminal   |    | SSH Stack      |     | Session Store  |                     |
| | Engine     |    | SFTP/Tunnels   |     | SQLite         |                     |
| +------------+    +-------+--------+     +----------------+                     |
|                           |                                                      |
|            +--------------+----------------+------------------+                  |
|            |              |                |                  |                  |
|     +------v-----+ +------v-----+  +-------v------+  +--------v-------+         |
|     | RDP Engine | | VNC Engine |  | Embedded X   |  | NX/X2Go-style  |         |
|     +------------+ +------------+  | Server       |  | Display Engine |         |
|                                     +--------------+  +----------------+         |
|                                                                                  |
|     +------------------------------------------------------------------+         |
|     | Fast GUI Plane                                                    |        |
|     | - Status widgets                                                  |        |
|     | - Clock                                                           |        |
|     | - Workspace switcher                                              |        |
|     | - Remote GUI surfaces                                             |        |
|     +------------------------------------------------------------------+         |
+----------------------------------------------------------------------------------+
```

---

## 5.3 Updated Module List

| Module ID | Module | New/Updated Responsibility |
|---|---|---|
| MOD-SESS-CTX | Session Context Menu | Right-click actions, bulk actions, delete/undo |
| MOD-SESS-STORE | Session Persistence Store | SQLite standalone storage, atomic writes, backups |
| MOD-XSRV | Embedded X Server | Bundled X server for standalone X11 support |
| MOD-ANX | NX-Compatible Display Engine | X2Go/NX-style remote GUI streaming |
| MOD-GUI-PLANE | Fast GUI Plane | Low-latency local/remote widget rendering |
| MOD-PACK | Standalone Packaging | Bundling of dependencies, runtime, assets, agents |
| MOD-AGENT-BOOT | Agent Bootstrap | Standalone remote agent deployment over SSH/SFTP |
| MOD-INTEGRITY | Integrity Checker | Component signature and checksum verification |

---

# 6. Updated Folder Structure Additions

The previous folder structure should be updated as follows.

```text
aetherdesk/
├── installer/
│   ├── windows/
│   │   ├── msix/
│   │   ├── msi/
│   │   └── portable/
│   ├── linux/
│   └── macos/
├── runtime/
│   ├── windows/
│   │   ├── flutter/
│   │   ├── libs/
│   │   ├── xserver/
│   │   ├── codecs/
│   │   └── fonts/
│   └── common/
├── core/
│   ├── rust/
│   │   ├── crates/
│   │   │   ├── ad-session-store/
│   │   │   ├── ad-session-context/
│   │   │   ├── ad-xserver-bridge/
│   │   │   ├── ad-anx-engine/
│   │   │   ├── ad-fast-gui-plane/
│   │   │   ├── ad-portable-storage/
│   │   │   └── ad-integrity/
├── agents/
│   ├── linux-agent/
│   │   ├── standalone/
│   │   ├── packaging/
│   │   └── bootstrap/
├── third_party/
│   ├── xserver/
│   ├── freerdp/
│   ├── codecs/
│   ├── sqlite/
│   └── licenses/
```

---

# 7. Updated MVP Impact

These additions change the MVP scope.

## 7.1 Scope Added to MVP

The following should now be considered part of MVP or near-MVP requirements:

| Feature | MVP Impact |
|---|---|
| Session save/delete context menu | Must be included in session manager MVP |
| Session soft delete/undo | Should be included early |
| Standalone packaging | Must be included from first usable build |
| Embedded X server | Required if X11 forwarding is included in MVP |
| Terminal-first main screen | Must be part of first UX foundation |
| X2Go/NX-style GUI engine | Required for remote GUI strategy, can be alpha/beta staged |
| Fast local widgets | Required for status line/clock/basic HUD |

---

## 7.2 Recommended MVP Adjustment

The MVP should now explicitly include:

1. Terminal-first application shell
2. Session manager with right-click context menu
3. Save/delete/rename/duplicate session operations
4. Local standalone storage
5. Portable/installed standalone mode
6. SSH terminal
7. SFTP
8. Basic tunnels
9. Embedded X server compatibility mode
10. Fast status line with clock and connection indicators

Remote Linux app publishing with X2Go/NX-style engine can follow as a high-priority post-MVP milestone if the team is small.

---

# 8. Updated Risks

| Risk | Severity | Explanation | Mitigation |
|---|---:|---|---|
| Embedded X server is large and complex | Very high | A full standalone X server is nontrivial | Use proven bundled X server component if legally acceptable; isolate as separate helper process |
| Licensing of X server/NX/X2Go components | High | Some components may be GPL or have complex licensing | Legal review; clean-room NX-style implementation if proprietary distribution is required |
| Standalone packaging increases installer size | Medium | Bundling all dependencies increases size | Accept larger installer; use compression and optional component download only if user explicitly enables it |
| Remote agent cannot be fully standalone on all Linux distros | High | GUI stacks vary by distro | Provide bundled libraries and graceful fallbacks |
| Terminal-first UX may conflict with rich GUI expectations | Medium | Users may expect dashboards | Keep GUI panels lightweight and optional |
| X2Go-style engine requires remote agent | High | Plain SSH-only environments may not allow agent | Provide X11 compatibility fallback |
| Performance-critical local widgets could still cause redraw overhead | Medium | Poor widget implementation can harm terminal performance | Enforce damage-based rendering and performance tests |
| Standalone updater complexity | Medium | Self-update must not break standalone model | Use staged updates, checksums, and rollback |

---

# 9. Updated Complexity Estimate

These additions increase the scope.

| Added Work | Additional Person-Months | Notes |
|---|---:|---|
| Session context menu/save/delete/undo | 1.5–3 | Mostly UI + persistence |
| Standalone packaging and dependency bundling | 2–5 | High importance, CI/packaging work |
| Embedded X server integration | 4–10 | Depends on whether proven component is reused |
| X2Go/NX-style GUI engine | 8–16 | Strategic but complex |
| Fast GUI Plane / terminal-first HUD | 2–5 | Performance-sensitive |
| Remote standalone agent hardening | 4–10 | Linux distribution compatibility |

Estimated additional total:

```text
21.5 to 49 person-months
```

If using existing components aggressively and limiting initial X11/NX scope, the lower end is possible.

If building a fully clean-room X2Go-like stack with embedded X server and robust standalone agent, the higher end is realistic.

---

# 10. Final Amended Requirement Summary

The following requirements are now mandatory additions to the AetherDesk document set.

## 10.1 Session Save/Delete Requirement

AetherDesk must allow sessions to be saved, edited, renamed, duplicated, exported, and deleted. Deletion must be available through a right-click context menu on the session name. Deletion must include confirmation, support undo/soft delete where enabled, and handle active sessions safely.

## 10.2 Standalone Requirement

AetherDesk must be a standalone application. It must not require external runtimes, external X servers, browsers, package managers, interpreters, or separately installed helper components. All required components, including terminal engine, SSH stack, SFTP engine, tunnel manager, RDP/VNC engines, embedded X server, codecs, fonts, plugin runtime, and session storage engine, must be bundled or embedded.

## 10.3 Terminal-First and X2Go-Style Fast GUI Requirement

The main AetherDesk screen must behave like a terminal workspace. The terminal canvas must be the primary interaction surface. GUI components must be lightweight and must not degrade terminal responsiveness. Remote GUI components and remote Linux application surfaces must use X2Go/NX-style technology or an equivalent damage-based, cached, compressed, GPU-composited streaming model to ensure fast response over WAN connections. Local widgets such as clock and status indicators must use low-overhead damage-based rendering and must not require heavyweight GUI frameworks or external web technologies.
