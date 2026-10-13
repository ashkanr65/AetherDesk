# File workspace and editor

The SSH, SSH+X11 and SFTP file panes now share file opening, editing, local application routing and file actions. These changes do not intercept clicks inside third-party X11 application windows.

## Everyday use

- Double-click a folder to navigate. Double-click a text/code file to open the built-in Monaco editor in a separate, resizable desktop window. **Editor** opens or focuses that connection's editor window. Additional documents use tabs in the same editor window; different host connections have separate editor windows. The main terminal and file panes remain available. SFTP-only connections use the full workspace for the two file panes.
- Use **Save to host** or Ctrl+S to save an editor document to its original host path. Ordinary local-pane documents save to their original local path. The editor preserves UTF-8/UTF-16 BOM and line endings; binary, invalid/ambiguous encodings and files over 8 MiB require an external application.
- For a GDS file, use **Open with…**, choose your installed KLayout executable, and optionally remember the association. Subsequent double-clicks use it. **Open with default program** uses the Windows association. **Use built-in editor for this file type** resets a type to the internal editor, subject to content validation.
- Files opened in an external application use a managed local working copy. Stable saves to that copy are sent back to the host while this connection and AetherDesk remain open. Status shows success, pending changes or a conflict. A recovered unsaved external draft waits for an explicit Save or a new application save before uploading.
- External **Save As** may produce another local file. Use the remote file's **Save local copy to host…** action to select it and its host destination. AetherDesk does not guess where a separately saved file should go.
- Drag files or folders between the two panes. Existing files require replacement confirmation. Transfers are queued, show progress, and can be cancelled or retried. Unsupported links and names that cannot be represented safely on Windows are reported instead of silently remapped.
- Right-click files/folders for opening, rename, delete, path copy, properties and permissions. File menus also provide comparison. These menus have no Download entry. Middle-click inserts a quoted host path into its owning SSH terminal without pressing Enter; SFTP-only connections have no terminal action.
- Host permissions accept Unix octal modes such as 644 and 755. Local properties/permissions open the Windows properties dialog. Link targets require explicit selection before editing.

## Conflicts and recovery

Host saves create and verify a sibling staging file, compare the original revision, preserve the available SFTP owner/group/mode, and commit through the server's atomic replacement extension. A server without that extension refuses replacement safely; saving a new copy remains available. Existing files are not resumed through an old partial upload.

When another writer changes a file, choose **Compare**, **Reload**, **Save a copy**, or explicitly **Overwrite**. Reload asks before discarding a draft. Failed reloads keep the working copy. Typing during a save leaves the later edit pending for the next save.

Closing the editor window offers **Save all and close**, **Keep drafts and close**, or **Cancel** when documents have unsaved changes. Closing that window leaves the SSH connection and external-application file watching running. Reopening **Editor** restores its document tabs. Disconnecting the connection closes its editor after handing over the latest draft. Use the native window close button to close the editor; the duplicate in-app title-bar button has been removed. Editor closing waits for WebView2 initialization and cleanup before destroying the native window. Repeated close requests share the same cleanup operation. The Windows stream-surface plugin is also registered in editor windows: its native destructor must check messenger availability before unregistering callbacks. Flutter invalidates the messenger before destroying plugins, so unconditional callback removal crashed the entire process even after WebView2 cleanup had completed. The native teardown regression covers this unavailable-engine case.

The October 1 editor-close fix was also checked with an isolated native harness using the packaged Flutter engine and assets: the original packaged stream-surface plugin crashed on the first child close with access violation `0xC0000005`; the corrected plugin completed three child closes while a separate keeper window remained alive, then closed the keeper successfully. The automated native regression exercises the production plugin over 100 child teardown cycles and checks that the main channel still responds. These are focused native tests, not a full application build or an SSH-session acceptance test.

Working copies and their recovery records live under `%LOCALAPPDATA%\AetherDesk\file-workspace\copies`. Closing an edited document offers saving or keeping a draft. Disconnect and normal application-exit requests preserve drafts and warn when saves are pending or external files are being watched. Reconnect and reopen the same host path to recover changes; the verified SSH host key, account, port and canonical path keep different servers separate. A forced process termination can still lose the last changes that have not reached the draft journal. Do not delete the working-copy directory when it contains pending work.

SFTP does not provide universal server-side compare-and-swap locking. A concurrent writer can still race the final replacement, and standard SFTP cannot preserve hard-link identity, ACLs or extended attributes. Files requiring those semantics should be edited with a host-side tool. Atomic host saves preserve the metadata explicitly described above, not every filesystem attribute.

## Optional language intelligence

Basic highlighting, indentation, folding, brackets, multi-cursor, undo/redo and search/replace work offline. **Language support…** starts an explicitly configured, already-installed language server on the SSH host. Enter its executable, a JSON list of arguments and the host project directory. This adds supported completion, diagnostics, definitions and symbol rename. A restricted SFTP-only account may not permit starting a language server. It is never required for ordinary editing, and this is not the complete VS Code extension platform.

Comparison supports a local or host counterpart, text differences and binary hash equality. Symbol rename is limited to files already open in the editor; server requests to create/delete files are not silently applied.

The optional P2 **local project copy** workflow for files with relative dependencies is not part of this first release. Single-file external opening does not automatically fetch neighbouring libraries/includes.

## Version and build

Edit the repository's `release.json`, initially version **0.1**, build **1**. For example change `version` to `0.2`. The app label, package names, Flutter version and agent verification version are synchronized from it; Git identifiers remain diagnostic source information. See [versioning.md](versioning.md).

The normal beginner kit now generates and packages offline editor assets and the WebView2 runtime installer. Node.js 22+ and npm are build prerequisites. See [file-workspace-packaging.md](file-workspace-packaging.md). Existing external kit launchers already delegate to the updated source scripts; their configuration and logs are preserved.

## Validation and remaining release check

Automated checks cover real SFTP staged saves/cancellation/conflicts, encrypted SSH input isolation and language-server transport, editor messaging, encodings, draft recovery, folder transfers, menus, and existing Flutter terminal/remote-desktop regressions. Version and packaging checks use isolated fixtures. Rust formatting is checked for the build-kit packages and changed backend crates. A broader `cargo fmt --all --check` also reports pre-existing formatting differences in unrelated placeholder crates; those were not changed by this feature.

No full AetherDesk, agent bundle, installer or portable application build was run for this implementation, as requested. Editor asset generation and focused checks/tests were run. Before distributing your build, test the Windows editor with typing, Ctrl+C/V/S, IME, different display scaling and opening/reusing/closing separate editor windows, cancelling close with unsaved changes, disconnecting with drafts, and two hosts open at once. Also test an actual GDS file with your installed KLayout, ordinary save, atomic-replace save, Save As, disconnect/reopen recovery, and the installer/portable editor on a machine initially missing WebView2. These native end-to-end checks remain the release gate; automated tests do not establish them.
