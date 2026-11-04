# Built-in Windows X11 runtime

Windows portable ZIPs and installers include a private `x11` directory beside
`aetherdesk.exe`. Users do not install VcXsrv, accept a second installer, or
download an X server on first launch. AetherDesk starts and stops its own
authenticated instance, with its tray icon hidden. Existing system X servers
are neither used nor modified.

The source-owned `tools/build-kit/Bundle-X11.ps1` is used by local build-kit
step 9 and GitHub Actions. It downloads the official, pinned VcXsrv x64 archive
and verifies SHA-256 before extracting it as data with 7-Zip. It never executes
the VcXsrv installer. The payload includes DLLs, the Visual C++ runtime,
software GLX, Xauthority tools, keyboard maps, locales and core fonts. Both
portable ZIP creation and installer creation include this entire directory.

The pinned version, binary/source URLs and SHA-256 checksums are recorded in
`tools/build-kit/toolchain.json`. The corresponding upstream source archive
(including component notices and build scripts) and license text are included
under `x11/licenses`. See the [upstream release](https://github.com/marchaesen/vcxsrv/releases/tag/21.1.16.1).
The first build downloads approximately 143 MB for the binary and source
archives; subsequent builds reuse the verified build cache. These downloads
happen on the build machine, not on the end user's computer.

### Native window painting (0.1.3 build 7)

The private multiwindow server explicitly uses `-engine 1 -compositewm
-nocompositealpha -nowgl +iglx`. Composite window backing must stay enabled:
disabling it caused blank application and dialog content in SSH+X11, despite
successful off-screen pixmap colour checks. Software GLX, application-visible
Composite support, authentication and the normal TrueColor visual remain
available. Only the optional per-pixel Windows desktop alpha path is disabled;
ordinary application RGB colours are not reduced to a limited palette.

The VcXsrv 21.1.16.1 access violation at RVA
`0x1f56d0` observed on Windows in build 6. In the matching bundled executable,
that address corresponds to a source read in the reverse pixel-copy loop that
matches `fbBlt`. The event record alone does not establish the caller, invalid
rectangle or lifetime error; the original crash still requires a process dump
or a matching reproduction for a source-level root-cause fix. Disabling native
alpha does not establish that this crash is fixed. Disabling Composite window
backing is not an acceptable crash workaround because it breaks real painting.

The native packaging regression starts an isolated authenticated instance with
the actual client options. It checks exact 24-bit RGB values after overlapping
copies in both directions, retains GLX/Composite capabilities, and repeatedly
moves and resizes owned top-level/child windows across root edges. It also
copies the pixel pattern into an actual native window and checks that window's
client pixels. The owned test window is briefly kept visible on the primary
screen without taking keyboard focus; no user windows are sampled. Off-screen pixmap
success alone cannot pass this gate. Required window backing is checked. These are
bounded regression checks, not a claim that the affected user's crash was
reproduced or all Cadence application behaviour was validated.

The build machine requires 7-Zip (available on the GitHub Windows runner).
`Bundle-X11.ps1` accepts `-SevenZip` and `-CacheDirectory` for other build
environments. Cache files are checked on every use; invalid files fail the
build rather than falling back to a system installation. A fresh package
directory is required, so files from an older runtime cannot leak into a build.

`.github/scripts/verify_x11_runtime.py` runs as part of the portable-runtime
check. It excludes installed X-server locations and SDKs from the child
environment, starts the bundled server on an unused display, checks cookie
authentication and the `fixed` core font, rejects a bad cookie, and stops only
its own process. Missing payload files or source archives fail the build.
The Flutter integration suite can also run against this bundle with
`AETHERDESK_X11_INTEGRATION=1` and `AETHERDESK_X11_PACKAGE` pointing to the package
directory (the parent of `x11`).

The pinned Windows server reads its authorization file in text mode, where
byte `0x1a` can be treated as end-of-file. Local cookies use cryptographic
rejection sampling to exclude this byte while retaining over 127 bits of
entropy. Every application startup also requires an explicit rejection of a
wrong cookie; otherwise AetherDesk stops that server and fails closed.

After rebuilding, distribute the new installer or the complete portable ZIP.
Copying just `aetherdesk.exe` is not a portable installation. A damaged or
incomplete package now asks users to repair AetherDesk, never to install a
separate X server.
