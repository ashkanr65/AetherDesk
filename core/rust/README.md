# AetherDesk Rust core

The Rust workspace provides the production SSH/SFTP transport and the C ABI
loaded by the Flutter Windows application.

## Implemented

- SSH password and key authentication
- known-host verification
- interactive shell and command execution
- SSH X11 forwarding requests
- SFTP listing, upload, download, and resumable transfers
- loopback-only local forwarding used by compatibility integrations
- embedded Aether Stream desktop lifecycle, H.264 decoding, input, resize, and text clipboard ABI

## Not implemented in the Rust core

`ad-rdp-bridge`, `ad-vnc`,
`ad-x11-bridge`, `ad-terminal`, `ad-terminal-render`, and `ad-tunnel` are
scaffolds. In particular, `ad_rdp_connect` and `ad_vnc_connect` return an
explicit not-implemented error. Do not use the presence of those crates as a
feature-completion signal.

## Validation

```powershell
cargo test --locked --release --target x86_64-pc-windows-msvc -p ad-ssh -p ad-ffi
```

The remote display decision and implementation gates are documented in the
repository root under `REMOTE_DESKTOP_PROTOCOL_DECISION.md`.
