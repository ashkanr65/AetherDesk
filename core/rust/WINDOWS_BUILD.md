# AetherDesk - Secure Remote Desktop Application

A modern, secure remote desktop solution built with Rust and Flutter.

## Architecture

AetherDesk consists of:
- **Rust Core**: High-performance, secure backend handling all remote connections
- **Flutter UI**: Cross-platform desktop interface
- **FFI Bridge**: Safe communication between Rust and Flutter

## Building for Windows 11

### Prerequisites

1. **Install Rust**
   ```powershell
   winget install Rustlang.Rustup
   ```
   Or download from https://rustup.rs/

2. **Install Visual Studio Build Tools**
   ```powershell
   winget install Microsoft.VisualStudio.2022.BuildTools
   ```
   Make sure to select "Desktop development with C++" workload.

3. **Install Flutter** (for the UI)
   ```powershell
   winget install Google.Flutter
   ```

4. **Install Git** (if not already installed)
   ```powershell
   winget install Git.Git
   ```

### Build Instructions

#### Step 1: Clone the Repository
```powershell
git clone <repository-url>
cd AetherDesk
```

#### Step 2: Build the Rust Core
```powershell
cd core\rust
cargo build --release
```

This will create:
- `target\release\aetherdesk.exe` - CLI application
- `target\release\ad_app.dll` - FFI library for Flutter
- Various `.rlib` files for the core libraries

#### Step 3: Build the Flutter UI (Optional)
```powershell
cd ui\flutter
flutter pub get
flutter build windows --release
```

#### Step 4: Create Standalone Executable

For a complete standalone Windows application:

**Option A: Using Cargo Bundle**
```powershell
cargo install cargo-bundle
cargo bundle --release
```

**Option B: Manual Packaging**
1. Copy `target\release\aetherdesk.exe` to your distribution folder
2. Copy `target\release\ad_app.dll` to the same folder
3. Include any required DLLs (check dependencies with `dumpbin /dependents`)
4. Create an installer using WiX, Inno Setup, or MSIX

### Running the Application

#### Run the CLI Backend
```powershell
cd core\rust
.\target\release\aetherdesk.exe
```

#### Use the FFI Library from Flutter
The `ad_app.dll` provides C-compatible functions:
- `ad_initialize()` - Initialize the core
- `ad_create_ssh_session(host, port, username)` - Create SSH session
- `ad_connect_session(session_id)` - Connect to session
- `ad_send_data(session_id, data)` - Send data
- `ad_receive_data(session_id)` - Receive data
- `ad_close_session(session_id)` - Close session
- `ad_get_version()` - Get version string

Example Flutter FFI usage:
```dart
final lib = DynamicLibrary.open('ad_app.dll');

final initialize = lib.lookupFunction<Int32 Function(), int Function()>(
  'ad_initialize',
);

final getVersion = lib.lookupFunction<Pointer<Utf8> Function(), Pointer<Utf8> Function()>(
  'ad_get_version',
);

initialize();
print(getVersion().toDartString());
```

### Distribution

To create a distributable package:

1. **Create a release folder:**
   ```powershell
   mkdir dist
   copy target\release\aetherdesk.exe dist\
   copy target\release\ad_app.dll dist\
   ```

2. **Include dependencies:**
   Use `dumpbin /dependents aetherdesk.exe` to check required DLLs.
   Most system DLLs are already on Windows, but you may need to include:
   - VCRUNTIME140.dll (Visual C++ Redistributable)
   - Any custom dependencies

3. **Create an installer:**
   - **WiX Toolset**: Professional MSI installer
   - **Inno Setup**: Simple EXE installer
   - **MSIX**: Modern Windows app package

### System Requirements

- **OS**: Windows 11 (or Windows 10 1903+)
- **Architecture**: x64 or ARM64
- **RAM**: 512 MB minimum
- **Disk**: 100 MB free space
- **Network**: Internet connection for remote sessions

### Development

#### Run Tests
```powershell
cargo test
```

#### Run with Logging
```powershell
$env:RUST_LOG="debug"
.\target\release\aetherdesk.exe
```

#### Build Debug Version
```powershell
cargo build
```

### Troubleshooting

**Build fails with linker errors:**
- Ensure Visual Studio Build Tools is installed with C++ workload
- Run: `vs_installer modify --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64`

**DLL not found at runtime:**
- Install Visual C++ Redistributable: 
  ```powershell
  winget install Microsoft.VCRedist.2015+.x64
  ```

**Flutter can't find the DLL:**
- Ensure the DLL is in the same directory as the Flutter executable
- Or add the path to system PATH environment variable

### License

MIT OR Apache-2.0

### Support

Visit https://aetherdesk.dev for documentation and support.
