# Build and package AetherDesk for Windows

This is the canonical Windows build guide. The GitHub workflow in
`.github/workflows/build-windows.yml` performs the same validation and creates
both a portable artifact and an installer.

## Requirements

- Windows 10/11 x64
- Visual Studio 2022 Build Tools with **Desktop development with C++**
- Rust stable with the `x86_64-pc-windows-msvc` target
- Flutter 3.47.1 (the version pinned by CI)
- Windows Developer Mode, because Flutter plug-ins use symbolic links
- 7-Zip on the build machine for extracting the pinned private X11 runtime

## Validate

```powershell
cd ui\flutter\ad_remote_desktop
flutter pub get
flutter analyze --fatal-infos
flutter test

cd ..\..\..\core\rust
cargo test --locked --release --target x86_64-pc-windows-msvc -p ad-ssh -p ad-ffi
```

## Build

```powershell
cd core\rust
cargo build --locked --release --target x86_64-pc-windows-msvc -p ad-ffi

cd ..\..\ui\flutter\ad_remote_desktop
flutter build windows --release

Copy-Item ..\..\..\core\rust\target\x86_64-pc-windows-msvc\release\ad_ffi.dll `
  build\windows\x64\runner\Release\ad_ffi.dll
```

These commands build the development client, not the complete distribution.
To produce a standalone package, use `tools/build-kit/Build-Everything.cmd`
with the build kit's configured tools, or the GitHub workflow below. The shared
packaging recipe adds the decoder libraries, matching Linux agent, and private
X11 runtime, then verifies them. Distribute the resulting installer or complete
portable ZIP, never just the `.exe` or the raw Flutter build directory.

## GitHub artifact

Push to `main`/`master` or run **Build AetherDesk for Windows** manually from
GitHub Actions. Download either:

- `aetherdesk-windows`: portable application directory;
- `aetherdesk-installer`: Inno Setup installer.

## Current packaging boundary

The package contains the Flutter client, `ad_ffi.dll`, decoder runtime, matching
Linux agent and a complete private X11 runtime. X11 startup does not depend on
VcXsrv installed elsewhere on the computer. Both the local kit and GitHub
Actions use `tools/build-kit/Bundle-X11.ps1`; see
[bundled-x11.md](docs/bundled-x11.md) for the checksum-pinned artifacts, license
and source bundle, and clean-path startup checks.
