# Native Rust desktop migration

This clone is under construction. It is NOT yet a feature-equivalent replacement.

The new workspace has a native Rust desktop preview and Rust ports of profile
serialization, tab/sidebar state, pointer-based resize calculations, physical key
mapping, lock state and legacy shell shortcuts. Existing core Rust crates are
preserved. No live connections are opened by the preview yet.

Production behavior is frozen against rust-migration.json and its reference
hashes. The Flutter/C++/Python code is kept as a migration reference until each
area has behavior and live-host verification. Removing it early would lose
features. The existing build kit still builds the reference Flutter application;
it is not a Rust-release recipe.

Manual development commands (no builds were run during migration setup):

    cargo test --manifest-path ui/rust/Cargo.toml -p ad-desktop-model
    cargo run --manifest-path ui/rust/Cargo.toml -p ad-desktop

The first command compiles the model regression tests; the second builds/runs a
clearly named development preview. Dependency lock generation and compiler/UI
verification are still pending. The preview imports saved production profiles
only on explicit request and never writes to production data.

Implementation order:
1. Persisted formats and isolated state; profiles/vault compatibility.
2. Direct asynchronous Rust SSH + complete terminal emulator/render/input.
3. SFTP workspace, transfers, file handlers, native editor windows.
4. Rust Windows presentation/clipboard/monitors/drag-docking + retained sessions.
5. Native Rust AI provider/tool loop with the same approvals and history format.
6. Rust release tools and installer/portable packaging.
7. Full feature parity, live-host checks, frame/input profiling and release gate.

Third-party libraries such as the Windows API, GStreamer and XWayland remain
external dependencies. A Rust application cannot turn those upstream products
into Rust. User scripts in PowerShell/POSIX remain supported data; they must not
be deleted as part of the implementation-language migration.

Framework reference: https://docs.rs/eframe/0.33.3/eframe/trait.App.html

## Preview build kit

The independent recipe is tools/rust-build-kit. Double-click Build-Everything.cmd
in the sibling AetherDesk-Rust-Beginner-Build-Kit folder. It runs preview tests
and creates a clearly named portable folder, ZIP and optional installer. The
legacy tools/build-kit remains the Flutter reference recipe.
