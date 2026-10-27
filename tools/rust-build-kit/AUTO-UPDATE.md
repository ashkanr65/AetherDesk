# Publish an AetherDesk auto-update yourself

After a completed build, double-click **Prepare-Auto-Update.cmd** in the beginner
build-kit folder. The Python entry point is **Prepare-Auto-Update.py** in the same
folder. No chat session is required. The launcher finds the sibling
`AetherDesk-Rust` source and `AetherDesk-Rust-output` folders automatically.

**Normal execution publishes a stable update for all users.** It does not merely
upload a draft. It never compiles again, installs the update, restarts AetherDesk,
changes a remote server, creates a branch or force-pushes.

## Each release

1. Before building, choose an unused version/build in the source `release.json`.
   Update the heading and changes in `RELEASE-NOTES.md` for that same version/build.
   Review the source changes you intend to make public.
2. Run **Build-Everything.cmd** and wait for all seven stages to pass. Test the
   completed application as appropriate; the publisher cannot establish live EDA,
   remote desktop or actual update/restart behavior automatically.
3. Double-click **Prepare-Auto-Update.cmd**. Keep its window open. It shows elapsed
   time, stages and upload/download progress. No repeated publishing approvals are
   requested: launching normal mode is the instruction to publish.
4. Wait for **DONE** and the GitHub release link. Existing users can use
   **Updates → Check now**. Records are in
   `AetherDesk-Rust-output/acceptance/auto-update-v<VERSION>-build<BUILD>/run.json`.

The current 0.1.3 build 3 is already published. Use a new build number for the next
release. Adding or changing this publisher is itself a build-kit input change;
an older completed build will correctly fail the source-fingerprint check.
The script does not change versions or relabel an existing executable.

## What happens automatically

- Require `main`, the matching GitHub origin, a public source repository and the
  existing signing key matching the pinned public key.
- Check passed build stages, package/source version, exact build fingerprint,
  nonempty Windows/Linux regression evidence, catalog hashes and bundled X11.
- Check installer filename and its numeric Windows version without executing it.
  Compare every package file with the original completed-build ZIP.
- Scan pending source additions for strong credential patterns, raw licence
  reports, private addresses, personal Windows paths and real-looking licence
  endpoints. Reject local settings/credential files and unexpected binaries.
  This is a pattern check, not a substitute for reviewing intended public code.
- Reuse the existing protected Ed25519 key to sign metadata, build the flat update
  ZIP, and verify every signed file/hash plus the matching installer copy.
- Commit the checked pending changes and push directly to `origin/main`.
  Stop if main diverged, source changed or the package is stale. No reset,
  automatic merge, new branch or force-push is used.
- Create/resume a matching draft, upload the exact assets and verify GitHub's
  hashes. Preserve mismatched or published releases instead of overwriting them.
- Publish the normal latest release, then verify the anonymous update feed,
  downloaded metadata signature and complete installer/ZIP download hashes using
  the same API routes as AetherDesk. Downloads are streamed rather than saved as
  additional large copies.

## Optional modes

Run these from a terminal in the beginner build-kit folder:

```powershell
.\Prepare-Auto-Update.cmd --check
.\Prepare-Auto-Update.cmd --draft
```

`--check` performs local preflight checks only. It uses no signing key, GitHub
credentials, commits, pushes or releases. Full original-ZIP/signature/download
verification occurs during normal/draft publishing as applicable.

`--draft` signs, verifies, commits, pushes source publicly and uploads assets, but
leaves a new release unpublished. Run normal mode later to publish the same draft.
Drafts are not offered through stable automatic updates.

For a deliberately portable-only build, add `--portable-only`. Normal mode
requires the completed installer; it never silently omits a missing installer.

You can run the Python file directly with the same arguments:

```powershell
py -3 .\Prepare-Auto-Update.py --check
```

## Publisher computer setup

Use the Windows account containing the original protected release-signing key
and the existing Git Credential Manager GitHub sign-in. The key is never exported
to a file, printed, replaced or committed. Ordinary users receiving updates need
none of these publisher tools.

Python 3.9+ and `cryptography` are needed on this publisher computer. The CMD
launcher prefers `AETHERDESK_PUBLISHER_PYTHON`, then the standalone publisher
environment below, then the older `%LOCALAPPDATA%\AetherDesk-Publisher`
environment, then installed Python. If none is configured, it can use the
existing bundled Python on this computer. That fallback calls no AI service.

For an independent environment, install Python with its Windows launcher, then
run once:

```powershell
py -3 -m venv "$env:USERPROFILE\.aetherdesk\publisher"
& "$env:USERPROFILE\.aetherdesk\publisher\Scripts\python.exe" -m pip install cryptography==46.0.3
```

The CMD launcher will find this environment on subsequent runs.
If folders were moved, set `AETHERDESK_RUST_SOURCE` to the source directory and/or
`AETHERDESK_RUST_OUTPUT_ROOT` to the build-output directory, or pass
`--output-root <directory>`.

## If something stops

Read the final **STOPPED** message. No error prints credential values or private
key material. Fix the cause, then run the same command again. Matching signed
files and draft uploads are reused; a matching published release is verified
without creating a duplicate. Only empty incomplete draft uploads may be removed
and retried automatically. Other mismatched assets are preserved for inspection.

If signing was interrupted before `update.json` was written, preserve/move aside
that incomplete release-output folder before retrying. It is never silently
deleted or accepted as a valid release. If publication succeeded but public
verification failed, the release may already be live: rerunning verifies it.

Do not generate a new signing key to fix a missing-key error. Use the original
publisher Windows account or recover the original key. A different key prevents
existing clients from trusting the update. The longer manual procedure remains
available in `RELEASE-GUIDE.html`.

The publisher holds the same Windows lock as the build kit, preventing a build
cleanup or another publisher from changing its inputs concurrently.
