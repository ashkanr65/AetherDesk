# AetherDesk updates

Application source and releases share the public `ashkanr65/AetherDesk`
repository and its main branch. The update address and pinned verification key
are unchanged, so existing installations keep working with the same feed.
Users need no GitHub account, token, Python, Node or separate updater installation.
Clients without this updater need one initial manual upgrade.

## User experience

Settings → Updates provides automatic checking (on), automatic downloading (off),
Check now, release notes, progress, Discard download and Update and restart.
Automatic checks start after ten seconds, then run at most daily. Updates never
force a restart. Downloads are remembered and reverified after restarting.

Installation requires confirmation and is disabled during active connections,
transfers, unsaved/busy editors and assistant/tool operations. Profiles, Windows
credentials, working copies and chats keep their existing data locations. Full
runtime packages include the matching Linux agent; the updater itself never
changes remote hosts. Verify compatibility with retained desktops before release.

Installer upgrades use the existing per-user Inno AppId and installation
transaction, disable forced application closing, and preserve failure logs.
Installer first-launch failure is reported for recovery; automatic rollback of
the installer after a failed first launch is not claimed.

Portable upgrades verify a flat ZIP against signed file lengths/hashes, back up
overwritten files, journal changes and restore on replacement or first-launch
failure. Only obsolete files from previous signed inventories can be removed;
unknown files remain. The helper copies its DLL dependencies to avoid locking the
DLLs it replaces. First UI render confirms startup. This is not a power-loss-proof
filesystem transaction: retain the recovery folder after interrupted installation.
Completed update directories are cleaned after a successful startup.
An installation lock also prevents updating a folder while another AetherDesk
instance from that folder is open; other instances are never forcibly closed.

### Portable recovery fix prepared for build 5

A real build 3 to build 4 upgrade on 20 October 2026 exposed a Windows
read-only-file bug: replacement failed on bundled Linux runtime files, and the
same overwrite operation stopped rollback before restoring all previous files.
The generic recovery dialog hid the failing path. Earlier package and feed
verification did not cover this actual upgrade failure.

Build 5 prepares each file beside its destination, flushes the complete bytes,
and clears a read-only attribute only on the package file it must replace or
remove. Windows permission/sharing errors on replacement receive a short bounded
retry. Incomplete writes preserve original bytes and permissions. Recovery tries
every journal entry, retains detailed errors and restarts the old version only
when recovery is complete (or no replacement began). Signatures, path checks,
installation locks and unknown-file preservation remain enforced.

Three required Windows filesystem tests cover read-only package/backup files,
an interrupted write and one locked recovery file. Build 5 compilation and a
real automatic two-version upgrade still need validation. Existing clients do
not gain this fix until upgraded; an affected portable folder may first need
manual repair using verified release files. Keep its update recovery folder.

The user's affected portable copy was repaired to the signed build 4 inventory,
then renamed to Documents/AetherDesk. Its read-only package attributes were
normalized, the failed pending-download reference was cleared, and the saved
version floor was preserved. All 7,158 files and the release signature verified;
the existing executable passed isolated rendering/editor checks and reopened
normally. This manual recovery is not proof that automatic restart now passes.
The portable folder name can remain AetherDesk across subsequent updates;
downloaded archives still carry versioned names for release identification.

## Signing and publication

The verification key and public download repository in `update-config.json` are
embedded at compile time. Signed metadata declares version/build, Windows x64,
one-year expiry, package SHA-256/length and every portable file. The client rejects
invalid signatures, expired/incompatible releases, downgrades, unsafe/duplicate
Windows paths, oversized packages and unexpected archive entries. Requests use
HTTPS with fixed GitHub API hosts. No publisher credential ships in the product.

The private Ed25519 key is protected in this PC's Windows Credential Manager as
`AetherDesk-Rust/provider/updates-release-signing`. The public project repository
also has its encrypted Actions secret `UPDATE_SIGNING_KEY`. No plaintext key file
is created or committed. Arrange an approved secret backup before retiring the
PC/repository; do not regenerate the key casually because existing clients pin it.
Windows Authenticode signing is separate and remains unconfigured.

Publisher-only Python tools use cryptography; configuring the Actions secret also
uses PyNaCl. Neither is an end-user application dependency.

1. Increase numeric version/build in `release.json` for each published release.
   Never reuse a pair. Update `RELEASE-NOTES.md`; commit and push main.
2. Manually run **Prepare signed AetherDesk update**, or use the local kit.
   The manual workflow runs Linux/Windows regressions and packaging and prepares
   an `aetherdesk-signed-update` workflow artifact in the public repository. It does not publish a release.
   The supplied Linux agent's version/catalog must match the Windows client.
3. For a local package, run `update_release.py prepare --package <folder>
   --installer <setup> --output <new-empty-folder> --notes-file RELEASE-NOTES.md`.
   Evidence/signing checks do not substitute for native/live acceptance.
4. Compile registered updater tests, then test real upgrades of disposable
   installer and portable copies: settings/credential preservation, failures,
   restart/recovery and remote compatibility. These checks have not run here.
5. Run `Publish-Update.ps1 -Directory <signed-output> -Python <publisher-python>`.
   It verifies signatures/hashes and creates a draft in public AetherDesk, uploading
   only the explicit package assets and metadata. The release tag targets the exact pushed source commit. Review
   before publication. Enable release immutability in public repository Settings
   → Releases if desired, before publishing the first release.

The 2026-10-19 user build of 0.1.2 build 1 passed all seven stages, including
compiler regressions and bundled X11. The portable archive matched all 7,158
package files and passed isolated rendering/editor-popup checks. The user-approved
[0.1.2 build 1 preview](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build1)
was published on 2026-10-19. All three remote asset SHA-256 digests match the local
signed release, and anonymous access was verified. Source was private at that time.
This preview is available for manual download and excluded from stable automatic
updates. Following explicit user approval, [0.1.2 build 2](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build2)
was published as a normal release for the update test on 2026-10-19. The anonymous
latest-release feed returns build 2, and the user's real build 1 successfully
detected it with Check now, displayed its notes and offered Download update.
No download, installation or restart was performed. Real two-version
installer/portable upgrade and broader R12 live acceptance remain pending.

## Build snapshot repair

Stages 2–6 execute from the hash-verified stage-1 snapshot, including the recipe
itself. Later checkout edits enter the next build; they do not discard this one.
Snapshot edits still fail validation. Changes during snapshot creation still stop
the build to prevent mixed source. The reported step-4 failure occurred while this
source was being edited; its bundled-X11 test passed. Restart after source work
finishes to include the updater. Failed-build caches remain available for retries.

Offline signature/package and snapshot fixtures passed. Native tests and real
installation remain for the user's build; formatting is not compilation.

## Build 3 publication — 20 October 2026

[AetherDesk 0.1.2 build 3](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build3)
is published as a normal latest release in the same public source repository.
The user's build passed all seven stages in 26:29, including Windows/Linux
regressions and bundled X11. The completed executable passed isolated dark/light
rendering and editor popup lifecycle checks. Package source inputs match main.

The existing Ed25519 key signed the metadata; all 7,158 portable files and the
installer copy were verified. GitHub reports matching lengths and SHA-256 digests
for all three assets. Anonymous latest-release and runtime metadata API requests
returned build 3, whose downloaded envelope verified with the pinned public key.
Older eligible clients can use the gear menu → Updates → Check now. A copy already
running build 3 correctly finds no newer update.

Evidence is under AetherDesk-Rust-output/acceptance/update-build3-20261020;
LATEST-RELEASE-CHECK.txt and LATEST-SIGNED-RELEASE.txt point to this release.
No application was compiled by the assistant and no user installation was
replaced. Real two-version upgrades, restart/recovery and data preservation
remain pending; feed verification is not an installation test.

## Build 4 publication — 20 October 2026

[AetherDesk 0.1.2 build 4](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build4)
is the normal latest update, including the Preferences layout repair. The user's
build passed all seven stages in 52:06, including both new rendered layout tests,
Windows/Linux regressions and packaged X11. Isolated dark/light/editor checks passed.
All 7,158 portable files, the installer copy and all three uploaded asset digests
were verified. Anonymous latest-feed and runtime metadata download checks passed;
the downloaded signature verifies with the existing pinned key.

Evidence: AetherDesk-Rust-output/acceptance/update-build4-20261020. The latest
signed-release and release-check pointers now refer to build 4. Older installations
can use the gear menu → Updates → Check now. Existing build 4 installations
correctly find no newer version. No user installation was replaced; real upgrade,
restart/recovery and data-preservation acceptance remains pending.


## 0.1.3 build 1 publication — 22 October 2026

[AetherDesk 0.1.3 build 1](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.3-build1)
is the normal latest release in the public source repository. The user explicitly
authorized publication with the known independent-window bugs, lifting the
previous publication hold for this release. Release notes mark that feature
experimental and retain its remaining limitations.

The completed user build passed all seven stages, Windows/Linux compiler
regressions and bundled-X11 verification. Package fingerprint
`851876908a16037cfd0bf102228af9c407139f16dd620b49a3671a8f237b2d51`
matched the build inputs before publication. Release source/tag commit is
`7539e8df0cd62370e3018aa19004f25b98dee87f`; source was pushed directly to main.
The subsequent documentation-only publication record does not alter build inputs.

The existing pinned Ed25519 key signed the update. All 7,181 portable inventory
entries, installer copy and three uploaded GitHub asset digests verified. The
explicitly approved packaged UI smoke check passed using isolated settings and no
remote connections or compilation. After publication, anonymous requests through
the runtime's latest-release and asset routes returned the new version and valid
signed metadata. Actual anonymous ZIP, installer and metadata downloads matched
their complete SHA-256 hashes.

Evidence: `AetherDesk-Rust-output/acceptance/update-0.1.3-build1-20261022`.
Latest signed-release and release-check pointers now refer to this release.
Existing installations can use gear menu → Updates → Check now. No user
installation, saved profile or live remote application was changed. Real upgrade,
restart/recovery, idle/wake and comprehensive detached-window acceptance remain
pending; publication does not establish complete feature parity or resolution of
the reported EOF/idle-hang root causes.
