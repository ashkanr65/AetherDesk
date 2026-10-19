# AetherDesk updates

Application source remains private in `ashkanr65/AetherDesk-Rust`. Public
downloads use `ashkanr65/AetherDesk`. The former project was retained as the
private, archived `ashkanr65/AetherDesk-Legacy-Private-20261019` repository.
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

## Signing and publication

The verification key and public download repository in `update-config.json` are
embedded at compile time. Signed metadata declares version/build, Windows x64,
one-year expiry, package SHA-256/length and every portable file. The client rejects
invalid signatures, expired/incompatible releases, downgrades, unsafe/duplicate
Windows paths, oversized packages and unexpected archive entries. Requests use
HTTPS with fixed GitHub API hosts. No publisher credential ships in the product.

The private Ed25519 key is protected in this PC's Windows Credential Manager as
`AetherDesk-Rust/provider/updates-release-signing`. The private source repository
also has its encrypted Actions secret `UPDATE_SIGNING_KEY`. No plaintext key file
is created or committed. Arrange an approved secret backup before retiring the
PC/repository; do not regenerate the key casually because existing clients pin it.
Windows Authenticode signing is separate and remains unconfigured.

Publisher-only Python tools use cryptography; configuring the Actions secret also
uses PyNaCl. Neither is an end-user application dependency.

1. Increase numeric version/build in `release.json` for each published release.
   Never reuse a pair. Update `RELEASE-NOTES.md`; commit and push private source.
2. Manually run **Prepare signed AetherDesk Rust update**, or use the local kit.
   The manual workflow runs Linux/Windows regressions and packaging and prepares
   a private `aetherdesk-signed-update` artifact. It does not publish a release.
   The supplied Linux agent's version/catalog must match the Windows client.
3. For a local package, run `update_release.py prepare --package <folder>
   --installer <setup> --output <new-empty-folder> --notes-file RELEASE-NOTES.md`.
   Evidence/signing checks do not substitute for native/live acceptance.
4. Compile registered updater tests, then test real upgrades of disposable
   installer and portable copies: settings/credential preservation, failures,
   restart/recovery and remote compatibility. These checks have not run here.
5. Run `Publish-Update.ps1 -Directory <signed-output> -Python <publisher-python>`.
   It verifies signatures/hashes and creates a draft in public AetherDesk, uploading
   only the explicit package assets and metadata. Source stays private. Review
   before publication. Enable release immutability in public repository Settings
   → Releases if desired, before publishing the first release.

The 2026-10-19 user build of 0.1.2 build 1 passed all seven stages, including
compiler regressions and bundled X11. The portable archive matched all 7,158
package files and passed isolated rendering/editor-popup checks. The user-approved
[0.1.2 build 1 preview](https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build1)
was published on 2026-10-19. All three remote asset SHA-256 digests match the local
signed release, and anonymous access was verified. Source remains private.
This preview is available for manual download and excluded from stable automatic
updates; no stable release is published. Real two-version installer/portable
upgrade and broader R12 live acceptance remain pending.

## Build snapshot repair

Stages 2–6 execute from the hash-verified stage-1 snapshot, including the recipe
itself. Later checkout edits enter the next build; they do not discard this one.
Snapshot edits still fail validation. Changes during snapshot creation still stop
the build to prevent mixed source. The reported step-4 failure occurred while this
source was being edited; its bundled-X11 test passed. Restart after source work
finishes to include the updater. Failed-build caches remain available for retries.

Offline signature/package and snapshot fixtures passed. Native tests and real
installation remain for the user's build; formatting is not compilation.
