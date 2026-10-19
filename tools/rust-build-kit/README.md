# AetherDesk Rust build kit

For the complete independent signing, publishing and update-test procedure,
open [AetherDesk build and release guide](../../RELEASE-GUIDE.html) in a browser.
A copy is included beside the beginner kit launchers as `RELEASE-GUIDE.html`.

This kit builds the native Rust client, including the Windows UI and
bundled Linux agent, X11 and video runtimes. It is not yet feature-equivalent to
the original application; see `RUST-FEATURE-PARITY.md` in the source or package.

Double-click `Build-Everything.cmd` in `AetherDesk-Rust-Beginner-Build-Kit`.
`Build-Portable-Only.cmd` omits the installer. All three launchers, including
`Rebuild-Everything.cmd`, automatically clear old generated outputs before running
the stages. After success they also remove staging files and caches to save disk
space, keeping completed packages, logs and reports. These commands compile and test the
source when **you** run them. No compilation was performed during this migration.

## Build computer

- Windows 10/11 x64, Rust 1.88+ through rustup, Visual Studio 2022 C++ tools and Windows SDK.
- GStreamer MSVC development/runtime SDK matching `tools/build-kit/toolchain.json`
  (currently 1.28.6), normally under `%USERPROFILE%/develop/gstreamer/1.28.6`.
  Set `AETHERDESK_GSTREAMER_ROOT` for a different installation path.
- WSL with the original build kit's Podman setup. The distribution defaults to
  Ubuntu; set `AETHERDESK_WSL_DISTRIBUTION` if needed. The Linux agent is built and
  tested in the original AlmaLinux container recipe, including bundled Xwayland.
- 7-Zip extracts the pinned X11 archive; its installer is never executed.
- Inno Setup 6 is required only for the installer stage.
- Internet access for initial downloads. Flutter, Node and Bun are not used by
  this native client recipe. Linux container build tools are installed inside
  the container.

## Stages

Explicit R12 live checks are available through `Run-Acceptance.ps1`, with a
non-secret configuration and a verified extracted package. They use disposable
host files/session IDs, existing keys in place, and isolated UI settings. Provider
calls require an explicit Provider selection and user authorization for the
destination/model/dummy payload. Normal builds never run these live checks.
Evidence stays in `output/acceptance`, which normal output cleanup preserves.
See the source `R12-ACCEPTANCE.md` for configuration and remaining manual checks.

0. Validate the reference/parity manifests and offline build-recipe fixtures.
1. Copy an isolated source snapshot; normalize Linux script line endings and
   native package versions; preserve/generate separate UI and core Cargo locks.
2. Check native formatting; compile/run native UI/model and production SSH,
   VPN, protocol, stream, decoder, QoS and session tests through their own workspaces.
   Require the cataloged compiler registrations and nonempty passing results.
   Build the Linux agent and run Wayland/supervisor/input/encoder/SSH tests, real local SFTP
   resume/conflict fixtures and interpreted SSH job cleanup fixtures in the container.
   The scoped-command suite also covers the Bash timed-read boundary, heartbeat
   renewal/expiry and malformed input. Its parser probes readiness without
   consuming data, then reads available bytes without a timer, avoiding Bash 4.4
   delimiter loss while preserving cancellation and owned-process cleanup.
   The AlmaLinux container installs the libav H.264 decoder and checks required
   video elements before compiling. Portable-only builds use this same test stage.
   Xwayland source downloads retry SSL/network failures with timeouts and an
   independent HTTPS mirror. Versions and SHA-256 checksums remain pinned;
   incomplete or corrupt archives cannot be extracted. Offline download fixtures
   run before the Xwayland build without network access or compilation.
3. Build the optimized native Windows executable, `AetherDesk.exe`, with the
   existing transparent product icon embedded as a Windows resource.
4. Package the executable, GStreamer/MSVC DLLs, X11 runtime, Linux agent/runtime,
   notices/licenses, lockfile, version and migration metadata. Run the native X11
   manager regression against that bundle: shared leases, authentication, fonts,
   fallback icon settings, cleanup and restart. Missing/zero-test results stop
   ZIP/installer creation. Matching Windows/Linux reports and catalog hashes are
   required and included under the portable/installer package's `verification/`
   and `agent/linux-x86_64/regression-results.json` paths.
5. Create the portable ZIP from that verified folder.
6. Create the separately identified Rust installer from the same folder.

Every stage has a fresh Windows PowerShell process and transcript. Failures stop
the sequence. Retrying a full or portable build starts again with fresh generated
outputs. To rerun an individual stage without clearing its prerequisites, use
`Run-Build-Step.ps1 -Step <number>`; saved records require matching source
fingerprints and artifact hashes. Changing source invalidates old stage records.
Source locks are copied, missing
locks are cached by dependency fingerprints, and subsequent Cargo commands use
`--locked`. Dependency locks still require review before a replacement release.

## Output

Generated Windows outputs stay under the sibling `AetherDesk-Rust-output`:
`packages`, `logs`, `state`, `source`, `cache`, `agent`, `reports` and `installer`.
`LATEST-FOLDER.txt`, `LATEST-ZIP.txt` and
`LATEST-INSTALLER.txt` point to completed packages. Staging directories exist
only while needed. After a successful full/portable build, cleanup removes
`source`, `state`, `cache`, `agent`, `installer` and temporary preflight/model
results. It preserves `packages`, `logs`, `reports`, `acceptance`, latest pointers
and unrelated user files. It also prunes unused container images bearing this
Rust build kit's ownership label; unlabelled or other projects' images remain.
`AETHERDESK_RUST_OUTPUT_ROOT` selects another separate output root if required.

At the start of a full or portable build, the kit removes old packages, installer
files, logs, reports, snapshots, stage records, preflight/model results and latest
package pointers. Save any old logs or packages you want to keep elsewhere before
starting. Caches remain available until the build succeeds, so a failed run can
still be diagnosed. Failed builds and individual `-Step` runs do not trigger
post-success cleanup. Cleanup runs under the build lock and stops on errors or linked artifact directories; it never
cleans the source folder. Individual `-Step` invocations keep existing outputs.

### Build speed

The Linux recipe keeps Rust compilation in a separate container cache layer,
before copying the UI connection helper, regression catalog and test scripts.
Changes confined to those later inputs rerun all required tests and packaging
without rebuilding the Linux Rust dependency graph. The first build with this
recipe must populate the new layer; backend/dependency changes still rebuild it.
For faster repeat builds when storage permits, set
`AETHERDESK_RUST_KEEP_BUILD_CACHE=1` before launching the kit. This opts out of
cache removal and retains Windows and labelled Linux caches. Staging files are
still removed after success. Alternatively, double-click `Build-Faster.cmd`
(installer and portable) or `Build-Portable-Faster.cmd` (portable only). The default
frees space and therefore makes later builds download/compile again. Starting
a full build still clears old generated outputs in either mode.

Fast mode uses more disk space and helps later builds after its first successful
run has populated the caches. Use the normal launcher to reclaim those caches
after a successful build. Required tests and release optimization stay enabled.

Every launcher now prints each stage's start time and elapsed `HH:MM:SS`, followed
by total build time, including preparation and cleanup. Failed builds also report
elapsed time. Worker times appear in each stage log; the latest structured report
is `AetherDesk-Rust-output/reports/build-timing.json`, updated after each stage.

Portable ZIPs now use fast compression. Their contents and runtime behavior are
unchanged, with a possible increase in ZIP size. Installer optimization and
release compiler optimization are unchanged. `Build-Portable-Only.cmd` also
avoids the installer stage when only a portable package is needed. No tests are
skipped by these speed improvements.

Installer and ZIP contain the same connection runtimes. The end user does not
install Rust, Flutter, Node, Bun, Python, GStreamer or VcXsrv on Windows. An OpenGL
3.3-capable graphics driver is required. Original profiles/credentials remain
untouched; the Rust client has separate application data and credential targets.
A user's explicitly configured SSH language server runs on their host as before.
AI/Tools SSH commands use Bash and util-linux `setsid` on the Linux host for
owned process-group cleanup. The Windows end user needs no additional runtime.

The numbered remaining-job table is in `RUST-MIGRATION.md`. R1–R9 source work and
the solid bronze/black and cream/gold themes are implemented; compiler/live/visual
verification remains pending. Editor grammars are embedded by the Rust dependency,
and queue/editor/terminal sources are included in the snapshot. The offline recipe
checks their presence and rejects theme background image loading. Stage 2 runs
editor Unicode/search/indentation/diff/completion and R6 physical-input/monitor/toolbar
regressions with the native tests. Use "File
transfers" for the persistent queue and "Recover working files" for retained
remote copies and drafts. Use "Saved desktop tabs"
to reopen duplicate desktop recovery bookmarks after a restart. Normal Disconnect
retains an attached desktop; confirmed End removes its duplicate recovery entry.

Remote desktop capture preserves physical Windows keys while its surface is
focused. Ctrl+Alt+Esc returns local keyboard control (and returns fullscreen to
the tab); the toolbar sends Ctrl+Alt+Del explicitly. Injected input remains
excluded. Drag its hamburger to float or dock on any monitor edge. Placement is
saved separately in `%APPDATA%/AetherDesk-Rust/remote-toolbar.json`; missing
monitors fall back to the primary display. Stage 2 runs the added Rust regressions
when you build. Live Windows input/DPI/toolbar checks remain under R12.

`Test-Recipe.ps1` runs offline fixtures only. Compiler command invocations inside
those fixtures are mocked. Formatting/recipe checks do not prove runtime parity.

R1-R11 source work is implemented. R12 compiler/live/clean-machine acceptance is
pending. `rust-regressions.json` covers 56 behavior groups and 37 original suites
and helper references. The build records Windows results under
`AetherDesk-Rust-output/reports/windows-regressions.json`; Linux results travel
with the agent. These reports always leave live acceptance and replacement
readiness false. Package naming does not change the recorded R12 acceptance status.


The AI/display follow-up adds registered checks for independent service identity,
bottom-anchored chat composition, mixed-DPI dock controls, exact-colour partial
texture uploads with full fallback, hidden terminal navigation, per-profile VPN
switching and bounded monitor metadata. The extended monitor message is appended
to the protocol; the legacy layout variant and payload remain unchanged. The
Xwayland patch exposes client physical monitor dimensions and a virtual refresh
rate capped to the compositor cadence. Monitor properties are sent only on real
changes. Native resolution, encoder quality and frame-rate defaults are preserved.
See SESSION-UI-TASKS.md and REMOTE-PERFORMANCE.md for remaining runtime checks.

Signed updates and publisher setup are documented in UPDATES.md at the repository
root. Application source stays private; public AetherDesk contains downloads only.
The release workflow is manual and never publishes automatically. Stages after
snapshot creation now run their recipe from that verified snapshot, so later
checkout edits do not invalidate a long packaging step. Snapshot tampering still
fails verification. Updater native/installation acceptance remains pending.
