# AetherDesk Rust preview build kit

This kit builds the native Rust development preview. The full SSH/SFTP/desktop/
editor/AI port is incomplete; this is not a feature-equivalent replacement.

Double-click Build-Everything.cmd in the sibling
AetherDesk-Rust-Beginner-Build-Kit folder. Build-Portable-Only.cmd skips the
installer. Rebuild-Everything.cmd reruns all steps while keeping caches.

Build-machine requirements: Windows 10/11 x64, Rust 1.88+ through rustup,
Visual Studio 2022 C++ build tools and Windows SDK, and Inno Setup 6 for the
installer. Internet access is needed for the first dependency/target download.
No Flutter, Node, Bun, Python, WSL or Linux agent build is used by this recipe.

Stages: preflight/reference checks, source snapshot+dependency lock, Rust model
tests/formatting, optimized native preview build, portable folder, ZIP, installer.
Each stage gets its own Windows PowerShell process and transcript. Failure
stops the sequence. Resume accepts only matching source and artifact hashes.

Outputs are all under the sibling AetherDesk-Rust-output folder:
packages, logs, state, installer scripts, isolated source snapshots, and cache.
LATEST-PREVIEW-FOLDER/ZIP/INSTALLER.txt contain verified package paths.
AETHERDESK_RUST_OUTPUT_ROOT can select another separate output folder.
A dedicated Cargo download and compiler cache stays in that output folder.

Version/build numbers come from release.json. Snapshot Cargo versions are
normalized without editing source. When source Cargo.lock exists it is copied;
otherwise the first snapshot resolves a lock cached by manifest/version hashes.
Code-only changes reuse that lock. Subsequent commands use --locked. This is a development workflow; dependency updates and a checked-in
lock must be reviewed before a feature-complete release.

The installer uses a separate Rust-preview application identity and directory.
It does not replace the production AetherDesk installation. End users of this
preview need no Rust or developer runtime. This preview uses OpenGL and needs a
working OpenGL 3.3 graphics driver. User application/profile data is not modified
by packaging; preview profile imports are explicitly read-only.

Packaging validates the Windows x64 executable and preview labels, and includes
Cargo.lock, build metadata and dependency license declarations/files.

No build has been performed during kit creation. Test-Recipe.ps1 runs only
offline recipe fixtures and does not invoke compiler commands.

Individual steps:
    ./Run-Build-Step.ps1 -Step 0
    ./Run-Build-Step.ps1 -Step 1
    ./Run-Build-Step.ps1 -Step 2
    ./Run-Build-Step.ps1 -Step 3
    ./Run-Build-Step.ps1 -Step 4
    ./Run-Build-Step.ps1 -Step 5
    ./Run-Build-Step.ps1 -Step 6
