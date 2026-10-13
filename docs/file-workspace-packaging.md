# Packaging the file workspace and editor

The embedded editor runs entirely from packaged Monaco assets. It does not load editor code, language workers, fonts or user files from a public CDN. Windows supplies the browser engine through the Microsoft Edge WebView2 Runtime.

## Shared build stages

The local kit and GitHub Actions call the same `Windows-Stages.ps1 -Stage EditorAssets` before checking/building Flutter. This runs `npm ci`, editor tests, `npm run build`, and an asset completeness check. Node.js 22 or newer with npm is a build prerequisite; add it to PATH or set `AETHERDESK_NODE` to `node.exe`. Node.js is not needed on a user's computer.

`ui/editor/package-lock.json` locks Monaco and its build dependencies. The generated flat directory `ui/flutter/ad_remote_desktop/assets/editor/` contains HTML, JavaScript, CSS, language workers, the icon font, license notices and manifest. Flutter packages that directory under `data/flutter_assets/assets/editor/`. Generated assets and node_modules are excluded from the source fingerprint; their source and lockfile are included. Step 4 records generated asset hashes, and Step 8 records the complete Flutter output.

The normal Flutter build compiles `webview_windows_plugin.dll`. Packaging explicitly includes its matching x64 `WebView2Loader.dll` from the plugin's NuGet dependency if Flutter's output does not already contain it. The plugin obtains its pinned NuGet SDK packages during the build; a fully offline first-time build therefore requires priming those caches as well as npm/Flutter/Cargo caches.

## Offline installation

`Bundle-Editor-Runtime.ps1` includes Microsoft's x64 Evergreen **Standalone** installer in `redist/`. It downloads once into `%LOCALAPPDATA%\AetherDesk\build-cache\webview2`, or uses `AETHERDESK_WEBVIEW2_INSTALLER` when that variable points to an already downloaded full installer. It refuses the small online bootstrapper, requires a valid Microsoft Corporation signature, and records the binary version and SHA-256 in `redist/webview2-manifest.json`.

This adds roughly 200 MB to the uncompressed package. It is downloaded during packaging, not when someone opens a host file. Clear or replace the cached installer when updating the bundled Evergreen version. The release captures the exact installer hash, while Windows can service the installed Evergreen runtime normally.

The AetherDesk installer detects the runtime in both per-machine and per-user registry locations. If missing, it installs the bundled runtime. The portable package contains `Install-Editor-Runtime.cmd` beside `aetherdesk.exe`; users can run it once if the editor reports that WebView2 is missing. That helper checks the package hash and Microsoft signature before installing. Neither route requires an internet connection on the target machine. A Windows administrator may be needed when local policy prevents installation; terminal/file-transfer features remain available.

The packaging checks reject missing editor workers, fonts, plugin/loader DLLs, the standalone installer, or a mismatched runtime checksum. They do not substitute for a release smoke test opening the editor on a Windows machine, including one that initially lacks WebView2.

Microsoft's supported distribution and detection mechanisms are documented in [Distribute your app and the WebView2 Runtime](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution).
