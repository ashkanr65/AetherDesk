# AetherDesk updates

Application source and releases share the public `ashkanr65/AetherDesk`
repository and its main branch. The update address and pinned verification key
are unchanged, so existing installations keep working with the same feed.
Users need no GitHub account, token, Python, Node or separate updater installation.
Clients without this updater need one initial manual upgrade.

## User experience

Preferences → Updates provides automatic checking (on), automatic downloading
(off), installation of downloaded updates on normal close (on), Check now,
release notes, progress, Discard download and Update and restart. Automatic checks
start after ten seconds, then run at most daily. Startup never installs an update.
Downloads are remembered and reverified after restarting.

With installation on close enabled, a completed verified download is installed
by a background helper after AetherDesk exits normally. The GUI stays closed;
open AetherDesk normally next time to use the new version. Downloading remains
an independent choice: enable automatic downloading or download an update manually.
Opt out by clearing **Install downloaded updates when AetherDesk closes**.
**Update and restart** remains the explicit immediate option.

Closing follows the existing connection/work cleanup flow. A busy or unsafe exit,
an incomplete download, a crash, or Windows shutdown/logoff does not start an
installation. The helper waits for the application to exit and briefly for other
windows using that installation; it defers if the folder is still in use. It
never forcibly closes another instance. If opened during replacement, wait for
the helper to finish before trying again.

Profiles, Windows credentials, working copies and chats keep their existing data
locations. Full runtime packages include the matching Linux agent; the updater
itself never changes remote hosts. Verify compatibility with retained desktops
before release.

Installer upgrades use the existing per-user Inno AppId, disable forced
application closing, and preserve failure logs. Both installer and portable
updates back up affected application files before replacement and verify the
installed signed inventory. Installed updates also back up the uninstaller files;
file rollback does not restore Windows uninstall registry state, so an interrupted
installer may still require installer repair.

Portable upgrades verify a flat ZIP against signed file lengths/hashes and journal
changes. Only obsolete files from previous signed inventories can be removed;
unknown files remain. The helper copies its DLL dependencies to avoid locking the
DLLs it replaces. This is not a power-loss-proof filesystem transaction: retain
the recovery folder after interrupted installation.

After an update on close, the backup, journal and RECOVERY.txt remain in its
LocalAppData/AetherDesk/updates/update-* folder until the user's next successful
rendered launch. Completion is acknowledged without opening a test GUI on exit.
If the new executable cannot start, close all copies and follow RECOVERY.txt to
run that folder's AetherDesk-updater.exe with **--recover-update** and its plan.json.
Recovery stays closed and preserves diagnostic evidence. Installer repair may
still be needed as described above. Existing Update and restart retains its
immediate first-launch check and portable rollback behavior. Completed update
folders are cleaned after a subsequent successful startup.

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

For the self-service local workflow, run `Prepare-Auto-Update.cmd` or
`Prepare-Auto-Update.py` from the beginner build-kit folder after a fresh completed
build. Normal mode verifies, signs, commits/pushes main, publishes and checks
anonymous downloads without another chat session. `--check` is local-only;
`--draft` uploads a new unpublished release. See
[AUTO-UPDATE.md](tools/rust-build-kit/AUTO-UPDATE.md) for prerequisites, safeguards,
retry behavior and the distinction between publication checks and live upgrades.

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

## 0.1.3 build 2 publication — 22 October 2026

[AetherDesk 0.1.3 build 2](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.3-build2)
is published as the normal latest automatic update in the public source repository.
The maintainer reports the completed build works and authorized update preparation.
All seven actual build stages passed in 01:24:04, including Windows/Linux native
regressions and bundled X11. Package fingerprint
`24b729d37290a9becdc129d3faf037cf96c50619915e76aa1b83e5e21b4e3f5c`
matched the current build inputs before signing and after the source commit.
The release tag targets `0b4c7c0f09c11255ec2e04d0e1735586c4e28014`, pushed directly to main.

The existing pinned Ed25519 key signed the update. All 7,181 portable files and
the installer copy verified. GitHub reported matching SHA-256 digests and lengths
for all three uploaded assets. Anonymous latest-release and runtime asset API
downloads returned build 2; complete installer, ZIP and metadata downloads matched
their signed local hashes, and the downloaded metadata verified with the pinned key.

Evidence: `AetherDesk-Rust-output/acceptance/update-0.1.3-build2-20261022`.
Latest signed-release and release-check pointers now refer to build 2. Existing
users can choose gear menu → Updates → Check now. No application was compiled by
the assistant, no installation was replaced, and no live remote work was changed.
Actual two-version upgrades/restart, clean-machine and wider EDA acceptance remain
separate. Release notes retain experimental independent-window limitations,
including lack of live viewing after minimizing the original application.


## 0.1.3 build 3 publication — 25 October 2026

[AetherDesk 0.1.3 build 3](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.3-build3)
is published as the normal latest release. The source was pushed directly to
public main at 69384c7536d72938d04ef11b4b5dce0f7cbd696a; the public release tag
resolves to that exact commit. The completed seven-stage build passed in 31:42
and matches source fingerprint
59c0f8d90bf93a8b8ab5a6e18162324d04b7be6a87808403fb1abaf55362b19f.
Windows/Linux compiler and bundled-X11 checks passed. Monitoring documentation
and fixtures use fictional server/account examples; local connection data,
credentials and the supplied raw licence report were not added to the release source.

The existing pinned Ed25519 key signed the metadata. All 7,181 portable file
lengths/hashes and the installer copy were verified. GitHub's three uploaded asset
hashes/lengths match the signed local files. Anonymous latest-feed access and full
metadata/installer/ZIP downloads through the application's asset API routes passed;
the downloaded metadata signature verifies against the unchanged public key.
Evidence is retained outside source under output/acceptance/update-0.1.3-build3-20261025.

This preparation did not compile again, replace a user installation, restart an
application or change a live server. Actual GUI update/restart, clean-machine
installation and wider EDA/Monitoring acceptance remain separate from these
publication checks. Experimental independent-window limitations remain documented.

## Crash evidence at the next startup (build 5)

A background reader checks up to 72 hours of recent Windows Application log
entries for this executable path and its bundled X11 server, using saved process
IDs and run times where available. It imports matching Error/Hang records and
correlated Windows Error Reporting metadata into windows-event-*.log files in
AetherDesk's existing Logs folder. It does not require administrator access,
change Windows logging, enable dumps, import command lines, or show startup dialogs.
Reports are deduplicated; imported Windows reports have seven-day retention.
If Windows records no usable event, there is no external crash evidence to import.
X11 failures separately preserve a bounded, redacted server-log tail before
its temporary runtime directory is removed.
