# Source-Owned Build Kit

`Windows-Stages.ps1` is the shared Windows recipe used by the local worker and `.github/workflows/build-windows.yml`. Flutter and Windows GStreamer versions live in `toolchain.json`. Linux packaging uses the repository's `agent/linux/Dockerfile.bundle` in both environments.

`Bundle-X11.ps1` is also shared by local step 9 and GitHub packaging. It extracts the checksum-pinned VcXsrv runtime into the package, including its libraries, fonts, keyboard data, license and matching source archive. The build machine needs 7-Zip; end users install nothing besides AetherDesk. The portable-runtime check starts this bundled server without an installed X server on PATH. See [bundled X11](../../docs/bundled-x11.md).

`Bundle-Assistant.ps1` packages embedded Python, LangGraph and its complete pinned dependency set for both portable and installer builds. Versions, official artifact URLs and SHA-256 checksums live in `assistant-runtime.json`. `Verify-Assistant.py` checks every packaged file, starts the runtime without developer tools on PATH, and runs the backend regression suite against that package. `Test-Assistant.ps1` verifies Flutter chat, approved local/SSH actions and reusable tools with a local model fixture. See [bundled AI Assistant](../../docs/bundled-assistant.md).

The external beginner kit is a launcher, not a second recipe. It delegates to this directory in the sibling `AetherDesk` repository. Override that location with `AETHERDESK_SOURCE` if needed. Each run reads the latest local working files, including uncommitted files, and their checksums invalidate old completion records. It does not fetch/reset Git or discard local changes. Pull the desired branch first when you want newer remote commits.

GitHub Actions checks out the triggering commit, including PR revisions, and calls the same Windows stages. The Linux and Windows artifacts therefore come from the same workflow revision, not independently fetched moving branch tips. Machine-specific setup stays separate: the local launcher uses installed Windows tools and Ubuntu; hosted jobs install their tools and build the Linux bundle on Linux.

All default local build outputs share one folder beside the source: with the source in C:\Users\rezae\Documents\Temp\Aether\AetherDesk, outputs go in C:\Users\rezae\Documents\Temp\Aether\AetherDesk-output.

- AetherDesk-localbuild/: generated compiler checkout and compiler caches.
- Dated Linux bundle/check folders inside AetherDesk-localbuild/: Linux outputs.
- Dated portable folders, portable ZIPs and setup EXEs: ready-to-use packages.
- logs/: per-step transcripts.
- state/: verified completion records.
- LATEST-*.txt: latest verified package paths.

The sibling beginner kit contains launchers and guides only for new runs. Its Repository-Recipe.ps1 must not force AETHERDESK_KIT_HOME to the launcher folder. Historical outputs, logs and records are preserved in their old locations; new paths invalidate their resume fingerprints. The first build using this layout starts in the new compiler checkout.

Local overrides: `AETHERDESK_BUILD_ROOT` (compiler checkout), `AETHERDESK_OUTPUT_ROOT` (all generated output; default build/log locations follow this root), `AETHERDESK_KIT_HOME` (logs/state/latest-package pointers), `AETHERDESK_GSTREAMER_ROOT`, and `AETHERDESK_PYTHON`. The default local setup still expects the existing Ubuntu account `ashkan` and its installed Linux dependencies. No full build runs merely by importing the common settings.

After installing a new client, save host work and explicitly end/recreate an old retained desktop to activate host-side changes. Reconnecting alone intentionally retains the running agent and applications.

Release numbering comes from the root `release.json`; see [versioning](../../docs/versioning.md). The editor adds Node.js 22+ as a build dependency and bundles its Monaco assets plus the Microsoft WebView2 offline installer; see [file workspace packaging](../../docs/file-workspace-packaging.md). `AETHERDESK_NODE` and `AETHERDESK_WEBVIEW2_INSTALLER` support custom/offline tool locations.
