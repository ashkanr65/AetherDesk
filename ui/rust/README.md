# Native Rust desktop migration

The native Rust interface calls the production Rust SSH, WireGuard, remote-session,
streaming and decoding backends directly. It now includes source implementations
for SSH+X11, remote desktop, popup editing, protected profiles, AI chat/actions,
Tools and logs. Full behavior parity remains incomplete and unverified.

See `../../RUST-MIGRATION.md`, `../../RUST-FEATURE-PARITY.md` and
`../../rust-feature-inventory.json` for implementation mappings and remaining work.
The Flutter/C++/Python reference sources are retained while the port is incomplete.
Upstream native dependencies such as GStreamer, VcXsrv and Xwayland are bundled;
rewriting the client in Rust does not rewrite those upstream libraries.

Use the sibling Rust beginner build kit for the correct toolchain environment,
separate workspace tests, Linux agent and portable/installer runtime packaging.
The legacy `tools/build-kit` recipe still builds the original Flutter reference.
The native recipe is `tools/rust-build-kit`.

For manual test invocation in an initialized build environment, use separate
workspace commands (the backend packages are not members of the UI workspace):

    cargo test --manifest-path ui/rust/Cargo.toml -p ad-desktop-model -p ad-desktop
    cargo test --manifest-path core/rust/Cargo.toml -p ad-ssh -p ad-vpn -p ad-stream-protocol -p ad-stream-client -p ad-display-decoder -p ad-remote-session

These commands compile. They have not been run by this migration pass. A bare
`cargo run` does not assemble bundled connection runtimes; use the build kit's
packaged folder for live remote desktop and SSH+X11 checks.

The Rust preview stores data under `%APPDATA%/AetherDesk-Rust` and uses separate
Credential Manager targets. Production profile import is explicit and does not
copy or alter production credentials. Saved native profiles can remember their
own credentials with optional master-password protection.

The Windows product executable is now `AetherDesk.exe`; Cargo uses the binary
name `aetherdesk`. The existing transparent ICO is embedded by the Windows SDK
resource compiler during normal builds, and installer shortcuts use that name.
