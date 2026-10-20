AetherDesk 0.1.2 build 5 — Portable update recovery

- Handle read-only bundled runtime files during portable updates and recovery.
- Flush each replacement to a temporary file in the destination folder before replacing the old file; incomplete copies preserve the original.
- Continue restoring other files when one recovery file is locked, and keep the failing paths and Windows errors in the recovery folder.
- Restart the previous portable application after a safely recovered replacement failure or failed archive extraction.
- Show horizontal tracks and filled progress on Application scale and Application font size sliders in Preferences.

Validation status:

- Build 4 passed its build checks, but the user's real portable update exposed a read-only-file failure affecting replacement and rollback. Those earlier checks did not prove successful upgrades.
- Three new Windows filesystem regressions cover read-only package files, interrupted writes and recovery with a locked file. They are required by the build kit.
- Build 5 compilation, native regressions and a real two-version automatic upgrade remain pending the user's build. No build 5 package has been published.

Updates use the existing Ed25519 verification key and ask before restarting. Windows Authenticode signing remains separate and unconfigured.

Existing failed update folders must be retained until recovery has been verified. Older clients with read-only installed package files may need repair or a manual upgrade before they can receive this fix automatically.
