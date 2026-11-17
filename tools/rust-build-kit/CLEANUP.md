# Task-scoped disk cleanup

The managed Rust build now gives every invocation a unique task. Source
snapshots use a short owner-marked directory; native WSL tasks use
`/tmp/ad/<task-hash>` so nested desktop Unix sockets fit Linux path limits.
The full project/task identity remains in the marker and ledger. Existing long
WSL task roots are still recognized for ownership-scoped recovery. Windows
snapshots, agent extracts, installer recipes and temporary test files live in a
marked `.cleanup-work/<short-id>` folder outside the checkout. Full builds clean
that folder in `finally`, on success **and failure**. Packages, ZIPs, installers,
logs, regression evidence and latest-package pointers are retained. Failed builds
keep unfinished packages, archives and installer output inside the owned task
directory; only verified completed outputs are moved into the retained packages
folder, without overwriting existing deliverables. Failed builds
restart from their checked source rather than keeping unbounded verification
bundles. Explicit single-stage results are retained as hand-off deliverables;
their cleanup report records that exception instead of silently breaking later
steps. The publisher continues to verify the final package and source fingerprint.

## Completed release retention

After a **successful full Build-Everything.cmd**, retain the newest completed
release set (package folder, portable ZIP and installer) and retire verified older
sets under `AetherDesk-Rust-output/packages`. **Build-Faster.cmd keeps older
completed releases**, using its existing `AETHERDESK_RUST_KEEP_BUILD_CACHE=1`
switch. Portable-only and separately requested stages also retain old releases.
Both entry points still clean temporary build/test artifacts and enforce the
same bounded reusable cache; the faster switch never disables that cleanup.

`Release-Retention.psm1` records complete release contents and SHA256 hashes under
`reports/completed-releases`, previews a dry run, then revalidates before applying.
Failed builds never retire an older release. Current outputs and latest pointers
stay intact. Old releases are moved by exact path into the current owner-marked
task's `obsolete-releases` directory, then removed by normal task cleanup.
An interrupted cleanup is recovered through the same task ledger.

This is explicitly authorized retention of generated release deliverables,
separate from disposable build files. Installed and running applications are
protected by installation paths, live executable paths, active-task claims and
Windows file locks. Changed contents, additional user files, links, source/Git
directories and uncertain ownership are preserved with a visible explanation.
Legacy releases without records are adopted only when package identity matches,
their ZIP contains exactly the same files/bytes/directories, and any installer
has matching AetherDesk product/version metadata. Arbitrary old/unnamed files,
copies elsewhere on the computer, incomplete or unverifiable packages are never
deleted. The latest result is in `reports/release-retention.json`; necessary
preservation exceptions are printed rather than silently deleting user data.

The Windows cache is reused at `AetherDesk-Rust-output/cache` and defaults to a
10 GB post-task budget. Only an owner-marked cache under an exclusive lease may
be trimmed. Compiler output is regenerable; dependency sources, Git repositories
and canonical notices are retained. A retained result, active compiler, linked
tree or protected cache larger than the budget produces a visible exception.
Existing nonempty unmarked caches require a reviewed adoption plan; age is not
ownership. Required runtime images and final user deliverables are outside the
cache budget. Layerless disposable Podman builds avoid accumulating intermediate
image caches in the first place.

## Inspect / recover

From this repository in PowerShell:

```powershell
& .\tools\rust-build-kit\Cleanup-Project.ps1 -Action Status
& .\tools\rust-build-kit\Cleanup-Project.ps1 -Action Recover -Project aetherdesk
# Review the exact recorded, inactive task resources, then:
& .\tools\rust-build-kit\Cleanup-Project.ps1 -Action Recover -Project aetherdesk -Apply
```

No resources are deleted by Status, DryRun, or Recover without `-Apply`.
Pre-policy build containers/images/targets are not automatically adopted.
OpenEDA and other tasks' resources are never cleanup candidates for AetherDesk.

## A task in a persistent PowerShell session

```powershell
Import-Module .\tools\rust-build-kit\Project-Cleanup.psm1 -DisableNameChecking
$task = Start-ProjectTask -Project aetherdesk -ProjectRoot $PWD.Path `
    -OutputRoot "$PWD\..\AetherDesk-Rust-output" `
    -CacheRoot "$PWD\..\AetherDesk-Rust-output\cache"
$failed = $true
try {
    # Put temporary work under $task.Task.work (Windows) or
    # $task.Task.wslWork (Ubuntu). Register retained outputs BEFORE use.
    Add-ProjectArtifact $task 'C:\absolute\requested-output.zip' -Role deliverable
    # Run work here. Do not create another Rust target directory.
    $failed = $false
} finally {
    Complete-ProjectTask $task | ConvertTo-Json -Depth 20  # review first
    Complete-ProjectTask $task -Apply -Failed:$failed | ConvertTo-Json -Depth 20
}
```

For a chat spanning several shell processes, `Cleanup-Project.ps1 -Action Begin`
creates a **manual** task and returns its ID, token and marked work paths. Keep
the token private to that task. Complete it with `-Action DryRun -TaskId ...
-Token ...`, review, then `-Action Complete -TaskId ... -Token ... -Apply`.
A manual task is conservatively considered active until explicitly completed;
an interrupted chat with uncertain ownership is reported, not guessed dead.
Do not pass another chat's token or use a short-lived shell PID as its owner.

Tasks/manifests/reports and the shared maintenance lock live under
`%LOCALAPPDATA%\AetherProjectMaintenance`. Both supported project IDs use that
lock. Builds hold a shared maintenance lease; compaction requires an exclusive
lease. Task IDs and full Podman IDs are recorded before exact-ID deletion.

## Podman commands

The verified existing store is Ubuntu, root, vfs,
`/var/lib/containers/storage`. No alternate store is created. Disposable builds
use `podman --storage-driver=vfs build --layers=false --rm=true --force-rm=true`
plus `AETHER_CLEANUP_PROJECT`, `AETHER_CLEANUP_TASK`, an owned `--iidfile`, and
labels `io.aetherdesk.project`, `io.aetherdesk.task`,
`io.aetherdesk.cleanup=disposable`. Temporary test runs use `podman ... run --rm`
with the same labels. Use `Add-ProjectPodmanResource` to record exact full IDs;
label-based **discovery** records owned failed-build leftovers too. Discovery
never authorizes a shared-label/global prune. Actual deletion is by exact ID
with `rm --force --ignore` or `image rm --ignore --no-prune` after inspection.
Running containers, uncertain Buildah activity, required image IDs and any image
referenced by another container are preserved. Unknown inspection results block
image deletion. Canonical licence notices and OpenEDA runtime images survive.

## Idle compaction

Routine cleanup does not compact. Only after a fresh human confirmation that
both chats and every other WSL job are idle:

```powershell
& .\tools\rust-build-kit\Cleanup-Project.ps1 -Action Compact -IdleConfirmed
# Read the helper, review its printed target/hash and registered Ubuntu disk.
& .\tools\rust-build-kit\Cleanup-Project.ps1 -Action Compact -IdleConfirmed `
    -ReviewedHelperSha256 '<reviewed hash>' -Apply
```

An active/uncertain task, unavailable exclusive lock, remaining container/build
storage, helper mismatch or missing Ubuntu registration defers/stops maintenance.
The sequence is sync, fstrim, WSL shutdown, reviewed compaction helper, Ubuntu
startup and image-preservation checks. Each command must succeed before the next.
Always report Windows' actual free bytes, not merely `df`'s virtual capacity.
