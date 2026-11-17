# AetherDesk project instructions

Existing user instructions, parent-directory instructions, source/design holds,
and approval requirements remain in force. These cleanup rules do not authorize
design edits, publishing, deleting source/history, or touching another chat.

## Persistent disk ownership and cleanup — every task

On November 16, 2026, accumulated Aether artifacts required removing 27 images,
29 build containers and a 7.4 GB cache. Cleanup and separate idle compaction
reduced Ubuntu's virtual disk from 456 GB to 148.6 GB and returned 307 GB to
Windows. OpenEDA projects, runtime images and licence notices were preserved.
Prevent a recurrence rather than relying on emergency global cleanup.

1. Before creating artifacts, import `tools/rust-build-kit/Project-Cleanup.psm1`
   and start a unique task with `Start-ProjectTask`. Use project identifiers
   `aetherdesk` and `openeda` only. The current repository is `aetherdesk`.
   Check `Recover-ProjectTasks` for interrupted work first, using a dry run
   before applying a reviewed plan. Do not adopt resources by age/name/tag.
2. Record temporary Windows/WSL directories, extracted packaging trees,
   verification outputs, duplicate temporary archives, container IDs and image
   IDs. Use the returned marked `Task.work` / `Task.wslWork` directories. Record
   deliverables and required runtimes explicitly before creating them. Use a
   manual task/token for chat work spanning shell processes; do not pretend a
   short-lived shell owns an ongoing chat. An uncertain/manual interrupted task
   must be reported and preserved until its owner explicitly completes it.
3. Before EVERY final response, including failure/interruption handling, review
   `Complete-ProjectTask` without `-Apply`, then apply that same task's cleanup.
   Remove proven disposable test/build containers, throwaway images, abandoned
   targets, packaging extracts and duplicate temporary archives automatically.
   Next-task recovery may remove only recorded, proven inactive artifacts.
4. Preserve canonical source code, Git history and uncommitted changes; design
   libraries and PDKs; credentials; canonical licence notices; required runtime
   images; requested deliverables; and anything claimed by another active task.
   Generated source/packaging copies are disposable only while their canonical
   source/notices remain outside the marked task directory. Never run Cadence
   library edits or old repair scripts as part of cleanup. Keep libraries/PDKs,
   credentials and irreplaceable work outside disposable workspaces.
5. Use ONE reusable cache per project, normally
   `../AetherDesk-Rust-output/cache` for this project, with a default **10 GB
   post-task budget**. Required runtime images and retained deliverables are
   excluded. All Windows Rust builds/tests use its `target` and `cargo-home`.
   Hold its exclusive cache lease while using it. Evict regenerable compiler
   output under a verified owner marker when necessary; preserve protected
   dependency sources/Git/notices and report any necessary budget exception.
   Fast-build switches do not permit an unlimited cache. Do not create another
   verification target or duplicate bundle for convenience.
6. Use the existing **Ubuntu / root / vfs** Podman store at
   `/var/lib/containers/storage`, verified by the helper before each operation.
   Do not open a second rootless/rootful store. Build with
   `--layers=false --rm=true --force-rm=true`, task/project/disposable labels and
   an owned `--iidfile`. Run temporary containers with `--rm` and the same
   labels. Record full exact IDs, including owned failed-build containers.
7. Remove only verified inactive containers with
   `podman rm --force --ignore <exact IDs>` and verified disposable images with
   `podman image rm --ignore --no-prune <exact IDs>`. Recheck labels, activity,
   image references and other active-task claims before deletion. Never use
   global prune/reset, shared-label prune, direct Podman-storage deletion, or
   unnamed/untagged/old status as disposal proof. Never clean another project's
   or chat's resources. Unknown pre-policy leftovers require ownership review.
8. Resolve absolute deletion targets and every existing ancestor first. Reject
   empty paths, filesystem/project roots, mount points, links/junctions and
   symlink escapes. Validate the entire tree before deleting any part. Windows
   deletion uses `Remove-Item -LiteralPath`; no broad wildcards or shell-built
   deletion commands. Temporary directory deletion stays inside an explicitly
   managed, owner-marked task directory.
9. Inspect actual Windows space with `Get-PSDrive C`; in the existing Ubuntu
   store inspect `df -h /`, `podman system df`, and `podman ps --all --external`.
   WSL's virtual free capacity is not Windows' real free space.
10. End each task with a brief report: what was removed, remaining cache size,
    actual Windows free space, ownership/budget exceptions, and any deferred
    compaction. Retain small manifests/reports for audit, outside disposable
    workspaces. See `tools/rust-build-kit/CLEANUP.md` for commands.

### Authorized completed-release retention

User instruction effective November 16, 2026: after a successful full
`Build-Everything.cmd`, keep only the newest verified completed release set in
`AetherDesk-Rust-output/packages` (folder, ZIP and installer). `Build-Faster.cmd`
must retain older completed releases. Failed, portable-only and single-stage
builds do not retire old releases. This explicitly supersedes preservation of
older generated release deliverables for this operation only; ordinary task
cleanup and the 10 GB reusable cache policy apply to both entry points.

Use `Release-Retention.psm1`: review its dry run, verify exact contents/hashes and
ownership, protect installed/running applications and other active tasks, and
move exact obsolete artifacts into the current managed task directory before
normal cleanup deletes them. Preserve modified/unverifiable files and links,
source/history, credentials, notices outside superseded generated packages,
and releases outside the configured packages directory. Never stop an app,
uninstall it, publish, or push as part of release cleanup. Report exceptions.

## Compaction is separate, idle-only maintenance

Routine task cleanup NEVER trims/shuts down/compacts WSL. Substantial reclaimable
space alone is not permission to interrupt jobs. Freshly confirm **BOTH chats
and ALL other WSL jobs are idle**, including unregistered jobs, before compacting.
Use the single shared maintenance lock across `aetherdesk` and `openeda`. If any
task is active or ownership is uncertain, defer and report it.

Use `Cleanup-Project.ps1 -Action Compact` first as a dry run. Read
the local compaction helper selected by `Cleanup-Project.ps1 -CompactionHelper`
and verify its disk target against Ubuntu's CURRENT HKCU Lxss registration, not
an old hardcoded assumption. Apply requires the reviewed helper SHA256 and
fresh `-IdleConfirmed`. Under the exclusive shared lock, run sequentially and
check each status before the next command:

```powershell
# Resolve the existing helper under the current Windows profile. If it lives
# elsewhere, supply its verified absolute path through -CompactionHelper.
$compactionHelper = Join-Path $env:USERPROFILE 'Documents\Codex\2026-10-19\run-x20\work\Compact-UbuntuDisk.ps1'
# Read this helper and verify its registered Ubuntu disk target before running
# the sequence below; the idle confirmation and shared lock are still required.
wsl.exe -d Ubuntu -u root --exec sync
wsl.exe -d Ubuntu -u root --exec /usr/sbin/fstrim -v /
wsl.exe --shutdown
& $compactionHelper
```

Afterward verify Ubuntu opens, all pre-existing required project/runtime images
remain, and actual Windows free space increased as expected. Stop on any failed
check. Do not rerun compaction during active development.
