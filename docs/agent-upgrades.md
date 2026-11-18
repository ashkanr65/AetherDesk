# Safe host-agent upgrades

Build 11 keeps running applications and older session runtimes intact. Each new
agent/runtime combination has a content address beneath
`~/.aetherdesk/agent/<version>/bundles/<identity>/`. The identity includes the
executable hash and the sorted library names, sizes and hashes. Several builds
with the same application version can coexist safely. Existing published
content that differs from its expected hashes is preserved and reported.

Connecting with the rebuilt client stages its bundled runtime automatically.
Reopening a retained desktop or application session continues using its running
agent. Disconnect/reconnect preserves the same agent. Replacing an executable
on disk cannot hot-upgrade a compositor that owns live application connections.

## Inventory and staging

The Linux agent bundle includes `agent-upgrade.py`. It uses Python 3 and the
standard library, installs no packages and never signals a process, closes a
session, changes input settings or removes an existing runtime. Run staging as
the owner of the sessions; it cannot stage into another account's home.

Read-only inventory of the current account:

```sh
python3 /path/to/linux-x86_64/agent-upgrade.py
```

An administrator can use `--all-users` for an inventory across accounts. Missing
permissions and unidentified processes are reported as unverified and preserved.
The inventory does not read endpoint tokens, credentials, shell environments or
application/design data. It enumerates processes once rather than polling.

Copy the complete verified Linux bundle to a location outside session/runtime
directories. Keep `ad-agent`, `libs`, `runtime-manifest.json`, `version.txt` and
`licenses` together. Review this dry run first:

```sh
python3 /path/to/linux-x86_64/agent-upgrade.py --bundle /path/to/linux-x86_64
```

Then stage that exact bundle for the current account:

```sh
python3 /path/to/linux-x86_64/agent-upgrade.py --bundle /path/to/linux-x86_64 --stage
```

Staging verifies all hashes and copies notices into a private temporary
directory, then publishes atomically without replacing an existing directory.
Concurrent staging attempts reuse the verified winner. On a failure, only the
current attempt's marked temporary directory is eligible for cleanup. Unknown
files, links, mount points and unrelated staging work are preserved. Interrupted
staging directories require ownership review; age alone never permits deletion.
The executable is never run by this tool. Its JSON report identifies preserved
processes, the new content identity and destination. `--all-users` cannot be
combined with `--stage`.

## Owner-controlled transition

1. Keep the older desktop and applications running while staging. Record the
   session IDs and executable hashes from the inventory before changes.
2. To try the updated agent immediately, right-click the saved connection in
   AetherDesk and choose **Open independent host session**. This creates a new
   session with the current bundle and saves its recovery identity. The original
   desktop and its applications stay where they are. Existing applications are
   not migrated between desktops.
3. When each owner is ready, they save their work, finish jobs and close their
   applications themselves. Only then do they explicitly use **End remote
   session** on that older session. Disconnect alone keeps the old agent alive.
4. Reconnect after the old session has ended to create it with the staged bundle,
   or continue using the independent session. Preserve older runtimes until their
   users and sessions no longer need them; this tool intentionally does not prune.

An occupied desktop returns a clear message rather than silently taking over
another viewer. Disconnecting the old **viewer** allows reopening that desktop
without ending its applications; it still uses its original agent. A separate
independent session is available when the original viewer must stay connected.

Older agents can continue imposing server-wide scanning load while they remain
running. The scanner fix cannot eliminate their load until owners safely retire
those sessions. This workflow prioritizes preserving work over an automatic
restart or forced live migration.
