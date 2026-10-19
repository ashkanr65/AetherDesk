# Rust session UI corrections — 2026-10-15

These changes apply only to AetherDesk-Rust. Compilation and package builds remain with the user.

| Item | Work | Status |
| --- | --- | --- |
| 1 | Password/passphrase input and secure credential saving in session form | Implemented; awaiting user build |
| 2 | Close control inside the existing tab size | Implemented; awaiting user build |
| 3 | Remove redundant tab arrows | Implemented; awaiting user build |
| 4 | Connection inputs and host verification in popup dialogs | Implemented; awaiting user build |
| 5 | Word-only terminal colours, comment lines and custom words | Implemented; awaiting user build |
| 6 | Stable bordered terminal settings sections | Implemented; awaiting user build |
| 7 | Adjacent action buttons | Implemented; awaiting user build |
| 8 | Single theme toggle; settings menu with logs, help and version | Implemented; awaiting user build |

Validation: source regression registry checks passed (56 behavior groups, 37 original suite mappings); changed Rust files passed formatting/syntax checks. Four new native regressions are registered for build-kit step 2. No compilation, native test execution or packaging was performed. Visual verification remains for the next user build.

## Follow-up fixes — 2026-10-15

| Item | Work | Status |
| --- | --- | --- |
| F1 | Save and open now saves credentials and session metadata; saved passwords load automatically, including reconnect; preserve server/account binding and master-password prompts | Implemented; native regression added, awaiting user build |
| F2 | Add 10px tab-label indentation without changing tab size | Implemented |
| F3 | Remove repeated connection heading/address; put connection actions and Terminal settings in one compact row | Implemented |
| F4 | Exclude Aether folder-navigation commands from Bash history; remove recognized older bootstrap entries without deleting user commands | Implemented; isolated Bash PTY checks passed |
| F5 | Erase startup prompt before redrawing it; avoid duplicating large startup scripts past the PTY input limit | Implemented; PTY checks passed, native regression added |
| F6 | Bottom-right Terminal settings actions, with Apply and save to the left of Cancel | Implemented |
| F7 | Visible slider rails, retaining dragging, scrolling and numeric controls | Implemented |
| F8 | Two settings columns; add custom words with + beside Colours | Implemented |
| F9 | Remove Trash checkbox from Sessions and Bookmarks | Implemented |
| F10 | Remove Preview from window title, executable, package, ZIP, installer and build output; clean old upgrade shortcuts/executable | Implemented; offline recipe checks passed |

New executable: `aetherdesk-rust.exe`. Latest package pointers are
`LATEST-FOLDER.txt`, `LATEST-ZIP.txt`, and `LATEST-INSTALLER.txt` under
`AetherDesk-Rust-output`. Existing packages are replaced when the user next runs
the full/portable build kit. Version and acceptance status are not promoted by
this naming change.

Verification: source registry and Rust formatting checks, offline PowerShell
recipe fixtures (Cargo mocked), and a real isolated Bash PTY test covering four
history configurations. Four native tests and one interpreted PTY check were
added to the build-kit regression coverage. No compilation, native test execution,
application build, installer build or packaging was run for these follow-up changes.

## Desktop, Files and latency fixes — 2026-10-15

| Item | Work | Status |
| --- | --- | --- |
| D1 | One horizontal session action row, including desktop monitor actions; horizontal scrolling for narrow windows | Implemented; awaiting user build |
| D2 | Remove the desktop typing instruction text | Implemented |
| D3 | Files opens in an independent native window for SSH, SSH+X11 and remote desktop; closing it keeps the connection running; SFTP-only sessions retain their workspace | Implemented; geometry/close regression registered |
| D4 | Lift the shared Windows UI font baseline by 1 point to center labels and close glyphs; cache font definitions | Implemented; visual check remains |
| D5 | Expose secure password saving in Connect, persist the choice and bound username after authentication, recall on reopening; preserve master-password protection | Implemented; vault/reload/opt-out regressions registered |
| D6 | Enter submits focused connection/settings/file/provider forms; inline single-line action fields submit their associated action; preserve terminal, chat, editor, menus and IME behavior | Implemented; focused-dialog regression registered |
| D7 | Prepare desktop pixels outside the GUI/input workers, retain one latest waiting frame, share prepared pixels with the renderer, avoid redundant idle redraws, and render only visible file rows | Implemented; frame-colour and isolated-update regression registered; live latency check remains |

The existing remote desktop profile inspected during diagnosis had password
saving disabled. Enable **Save password / passphrase securely for reconnects**
once in its Connect dialog, enter the password, and connect successfully. The
new choice is then saved for reopening from the main Sessions list. No existing
user profile or stored password was changed during this work.

Verification: source registry/migration checks and offline build-kit fixtures
(Cargo mocked) passed. Five new native behavioral tests are registered in the
existing build-kit step 2, and the snapshot checks include both new modules.
No compiler, native tests, application, installer or ZIP build was run. Native
test execution, popup/alignment inspection and real host latency measurements
remain for the user's next build; source fixes are not a measured performance
result or an R12 acceptance pass.

## Build speed and main-screen label

| Item | Work | Status |
| --- | --- | --- |
| B1 | Cache Linux Rust compilation separately from UI/catalog/helper test inputs; continue executing all required tests | Implemented; offline recipe checks passed; next build populates the new cache layer |
| B2 | Use fast portable ZIP compression, retaining the same files and release optimization | Implemented; PowerShell recipe checks passed |
| B3 | Remove the version from the main toolbar; retain it in Help/About and package metadata | Implemented; awaiting visual check |

Build speed has not been timed because no compilation or packaging was run.
Existing cleanup still clears generated outputs and preserves compiler/download
caches. Keep the output cache and Podman's WSL cache to benefit on repeat builds.

## Build-kit Enter regression correction — 2026-10-15

The user's native run reported 107 passing tests, one failing dialog test and
five intentionally ignored tests. The dialog fixture sent repeated Enter
key-downs without key-up events. egui reconstructs held-key repeat state itself,
so its final supposedly fresh Enter became a repeat and was correctly rejected
by the form handler.

That same native log confirms the new frame-colour/latest-frame, Files geometry
and close-state, password opt-in/reload/revocation, and bound-profile update
regressions passed. This does not replace live visual or host-latency checks.

The fixture now sends key releases between independent presses and explicitly
ends its simulated IME composition. It also checks that holding Enter does not
submit twice and that a new press after release does submit. The production
repeat/modifier/focus guards and the required build-kit test remain enabled.
Formatting and offline recipe/source checks passed; native rerun remains for
the user's next build. No compilation or package build was run for this fix.

## SSH heartbeat cancellation correction — 2026-10-15

| Item | Work | Status |
| --- | --- | --- |
| H1 | Avoid Bash timed reads consuming heartbeat bytes; probe readiness and read one available byte without a timeout, preserving delimiters | Implemented; all 10 script checks passed in both the cached AlmaLinux/Bash 4.4 build environment and Ubuntu/Bash 5.3 |
| H2 | Deterministic consumed-delimiter/timeout regression; malformed, oversized, unterminated and NUL heartbeat cleanup checks | Added to the existing automatically discovered build-kit script suite |
| H3 | Full application/native/package build with this embedded supervisor | Remains for the user; no compilation or build was run |

The old parser was reproduced cancelling a valid lease with exit 125 when a
timed read consumed the delimiter before reporting timeout. Simply preserving
partial characters still failed under the actual AlmaLinux Bash 4.4, so the final
parser avoids interruptible consuming reads entirely. Its readiness probe
consumes nothing. Byte reads use the C locale and a NUL delimiter so malformed
input cannot leave the supervisor blocked waiting for another character.

The ten-second lease, strict heartbeat validation, EOF cancellation, command
exit status and cleanup of only the owned process group remain. Tests use
disposable local jobs and the existing container image with networking disabled;
no SSH host, real command history or user files are involved. Source/offline
recipe checks also passed. Run the usual build kit again to embed the fix.

## Compact controls and storage cleanup — 2026-10-15

| Item | Work | Status |
| --- | --- | --- |
| U1 | Reset dock width and height when changing edges/DPI; keep drag identity stable | Source implemented; native orientation regression registered |
| U2 | Vector icons for monitor, all monitors, return to tab, secure keys and disconnect; retain tooltips | Source implemented; visual check pending |
| U3 | Files pressed-state button opens/closes its native popup; vector folder/file rows avoid missing glyphs | Source implemented; runtime check pending |
| U4 | Remove New Connection beside the main gear; preserve saved-session + and keyboard shortcut | Source implemented |
| U5 | Embed existing transparent Windows icon and name packaged executable AetherDesk.exe; update portable/installer/acceptance paths | Recipe implemented; compilation/package check pending |
| U6 | Switch to Connections after selected tab authenticates; put disconnect icon beside its name | Source implemented; live check pending |
| U7 | Bound sidebar content to shared user-resized width and persist resize/reset | Source implemented; native oversized-content regression registered |
| U8 | Put AI model/provider/workspace/history controls in its gear popup; retain messages, composer, capture and approvals in sidebar | Source implemented; visual/provider check pending |
| U9 | Highlight hovered tabs and their separate close targets | Source implemented; existing close-target test retained |
| U10 | After successful full/portable build, remove staging/download/compiler caches and only unused Linux images labelled as Rust-kit-owned; retain finished packages/evidence/user files | Implemented; offline safety fixtures added; actual cleanup runs only when user builds |

Native compilation, UI tests, Windows resource compilation, real application
inspection and package generation remain for the user. No build was run for
these changes. Failed builds keep their temporary files for diagnosis. By
default successful builds free caches, so later builds take longer; optionally
set AETHERDESK_RUST_KEEP_BUILD_CACHE=1 to retain caches instead. Historical
unlabelled container images are deliberately not removed automatically.

Verification for U1-U10: Rust formatting/parser checks, the 56-group source
regression registry, migration references and offline PowerShell recipe checks
passed. The recipe suite exercised disposable cleanup folders, root/junction
rejection, preserved packages/logs/reports/acceptance/latest pointers and a
mocked label-filtered Linux cleanup command. Cargo and WSL cleanup are mocked
in these checks; no real build cache was deleted or native test run.


## AI, display and connection follow-up — 2026-10-15

| Item | Work | Status |
| --- | --- | --- |
| V1 | Independent saved AI services, explicit Edit/Remove, searchable provider/model chooser and bounded service list | Source implemented; identity regression registered; live API/UI check remains |
| V2 | Composer anchored to sidebar bottom, model chooser left and Send/Stop right; orderly gear settings | Source implemented; headless layout regression registered; native run remains |
| V3 | Exact-size vector dock controls across mixed DPI and orientation changes | Source implemented; expanded native dock geometry regression registered |
| V4 | Suppress identical decoded images and upload only changed pixels when the texture base matches; complete-frame fallback after skips/resizes | Source implemented; colour/fallback regression registered; live performance comparison remains |
| V5 | Hide token-framed internal directory commands while preserving real errors, directory synchronization and user history | Source implemented; interpreted real Bash PTY check passed; native split-stream filter regression registered |
| V6 | AetherDesk window title; saved-session labels left aligned and vertically centred | Source implemented; visual check remains |
| V7 | Send client monitor DPI, millimetres and refresh metadata on changes, retain legacy layout wire format, expose virtual display properties to host | Source implemented; protocol/native RandR regressions updated; verified Xwayland patch application; native/package run remains |
| V8 | Explicit remote virtual resolution settings; no automatic resolution, quality or frame-rate reduction | Source implemented; geometry regression registered; host desktop preference check remains |
| V9 | WireGuard Browse, enable/disable and Remove, persisted per profile; disabled configuration uses direct transport for that tab | Source implemented; configuration/legacy-profile regressions registered; native file-picker and VPN live checks remain |
| V10 | Remove duplicate lower settings gear and align/explain New Connection shortcut | Source implemented; visual check remains |
| V11 | Compare X2Go transfer architecture with live AetherDesk source and identify EDA-safe priorities | Analysis completed in REMOTE-PERFORMANCE.md; actual side-by-side measurement remains |

Verification is limited to Rust formatting/parser checks, source regression and
migration references, offline recipe fixtures, an interpreted local Bash PTY
fixture, and patch application against the checksum-verified pinned Xwayland
source. No compilation, native Rust tests, application builds, remote-host
changes or performance benchmark were performed. The build kit retains all
registered native tests for the user's next build. R12 and replacement readiness
remain incomplete.

Colour acceptance explicitly includes Cadence Virtuoso 2025: full RGB UI updates,
one-pixel coloured geometry and actual application reference comparison. The
reported X2Go colour problem has not been reproduced; current H264/NV12 is not
a lossless EDA path. See REMOTE-PERFORMANCE.md for the remaining acceptance work.


## Composer regression correction — 2026-10-15

| Item | Work | Status |
| --- | --- | --- |
| V2a | Measure the real composer frame rather than selecting painted rectangles (which mistakenly selected the full-window background) | Corrected in source; test retains resize/restore coverage and now checks compact height, bottom anchoring and both horizontal edges against the panel content bounds |
| V2b | Re-run the native composer regression and full build-kit tests | Pending user's build; no compilation or native test was run for this correction |

The production layout and input behavior are unchanged. The rendering methods
return their actual composer frame bounds so the regression can observe the
specific component. The test no longer assumes a hard-coded panel margin or
identifies components by their background colour/paint order.


## Connection controls, keyboard and AI layout — 2026-10-15

| Item | Work | Status |
| --- | --- | --- |
| W1 | Vector folder, disconnect, terminal-settings and end-session controls with tooltips | Source implemented; reuse disconnect consistently; native visual check remains |
| W2 | Add separate Disconnect / End session controls to the fullscreen/all-monitor dock; resize its orientation-aware geometry | Source implemented; expanded existing seven-control DPI/orientation regression; native run remains |
| W3 | Stable desktop focus identity, real native-route ownership checks, scoped Windows-generated/virtual-only key routing, host shortcut ownership instead of application navigation/zoom | REOPENED: user confirms keyboard still fails in both modes; source patches were insufficient; compare repair choices before further keyboard changes |
| W4 | Per-tab two-way directory-sync switch in SSH file popup; disabled state drains OSC notifications, invalidates pending mirrored listings and cancels unsent folder commands | Source implemented; both-direction, delayed-update and cancellation regressions registered; live SSH check remains |
| W5 | Compact composer with attachment left, model selection beside it, Send at right on one fixed-height row; uniform settings section borders | Source implemented; alignment and existing bottom-anchor/resize regressions registered; native visual check remains |
| W6 | Explain absent reasoning options; honour provider-default effort, supported-efforts null vs omitted metadata, and mandatory reasoning restrictions | Source implemented; capability regression registered; provider/live UI check remains |
| W7 | Display ordinary Windows/UNC paths, retain canonical scope identifiers internally, simplify Windows/host workspace selection | Source implemented; path/scope regression registered; native visual check remains |

Keyboard remains scoped to the focused remote surface and its foreground window.
Focus loss, lease expiry, disconnect and Ctrl+Alt+Esc release held input. The local
Windows secure attention sequence cannot be redirected by a normal application;
the existing explicit Ctrl+Alt+Delete remote control remains available.

Directory synchronization is local to each SSH tab. The terminal settings option
sets the default for new SSH tabs. Disabling synchronization cannot undo a cd
already executed remotely, but prevents future changes, queued unsent changes and
pending mirrored file listings from moving the other view.

Reasoning capability parsing follows the provider's documented metadata:
[OpenRouter reasoning options](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens).
Missing options are explained rather than invented. No API requests, remote-host
commands or library changes were made for these fixes. Native compilation,
application execution, builds and live acceptance remain for the user.


Reasoning requests also honour mandatory-model restrictions outside the settings
window. Explicit Off overrides a saved token budget, and any reasoning budget
leaves at least one token under the visible reply limit. A registered request-body
regression covers these cases; it makes no API calls.

W1-W7 source verification: formatting/parser checks, regression/migration source
references and the offline recipe fixtures passed (Cargo is mocked in the recipe).
The new Rust regressions and live keyboard/UI checks have not been executed.


## Keyboard review and alignment corrections — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| X1 | Reopen the critical keyboard bug; audit both shared input paths and compare practical repair backends | Live ordinary and shifted typing confirmed in remote Mousepad. User confirmed Ctrl+Q restores terminal typing: paused terminal output is the immediate cause. User chose session-scoped prevention. Linux terminal policy is implemented in source; native regression and rebuilt-package acceptance pending. No keyboard-backend rewrite applied. See REMOTE-PERFORMANCE.md |
| X2 | Align the WireGuard label with the path/Browse row; move Enable/Remove to a separate grid row beneath the path | Source corrected; native visual check pending user's build |
| X3 | Remove the duplicate saved-password sentence at the bottom of session settings | Source corrected; password-field hint retained |
| X4 | Match actual rendered attachment/model/send control heights and text centres; account for larger fonts; retain the searchable model popup | Source corrected; new regression measures the actual rendered control responses instead of estimated row rectangles; native run pending |

No additional keyboard fix or backend switch has been applied in this review.
After window control recovered, a live check through the computer-use skill
confirmed `a` and `Shift+B` arrive as `aB` in the unsaved remote Mousepad editor.
The user reports the failure in the remote terminal instead. No remote shell
input, EDA work, file save or connection lifecycle was changed. Read-only SSH
inspection timed out; the user then confirmed Ctrl+Q restores terminal typing. The user chose to prevent output pausing while preserving other shortcuts.
That policy is now implemented in source, as recorded below. No automatic terminal reset or keyboard backend change was made.

Formatting/parser checks, regression/migration references and the offline
build-kit recipe fixtures passed. Cargo is mocked in those recipe fixtures;
no application or native Rust regression was compiled. The new rendered-control
alignment regression is registered for the user's next build. UI changes require
a new build before they appear in the running portable package.


## Session-only terminal output policy — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| Y1 | Prevent Ctrl+S from pausing terminal output in remote desktop sessions | Source implemented in agent-wayland. A desktop-lifetime background worker clears only IXON on same-user PTY inputs whose process DISPLAY and private XAUTHORITY both match this desktop. It resumes already-paused output when making that change; it never reads/discards terminal input. Native/live checks pending user's build. |
| Y2 | Preserve other shortcuts and isolate unrelated terminals | No GUI keyboard filtering, shell rc edits or global terminal preferences. Exact environment entries, UID, process start time and input-device identity gate changes. Three Linux regressions registered: real PTY output/shortcut bytes/other flags; matching desktop versus unrelated terminal and worker shutdown; exact identity matching. Registered, not executed. |
| Y3 | Integrate with the existing build kit and record validation | Existing Linux agent-wayland test/list commands pick up the new registered regressions. No new build step, dependency or installer/portable distinction is needed. Source validation results recorded below. |

The worker scans periodically every 500 ms outside input/rendering. New PTYs or
programs restoring IXON are corrected on a subsequent scan; this is not an
instantaneous terminal-creation hook. Unreadable/hidden process information is
skipped without changing that terminal. The guard stops and joins when its
hosted desktop is destroyed. Disconnecting from a persistent desktop leaves
its running agent/policy in place; ending that desktop ends the guard.

No host commands, host preferences, credentials, Cadence files or existing live
terminal settings were changed during this implementation. The current open
package retains its original code; reconnecting alone does not load this fix.
Build the updated package/Linux agent, then start a new remote desktop session
to verify Ctrl+S no longer leaves the terminal paused. Check ordinary typing,
Ctrl/Alt shortcuts, terminal applications and unrelated host terminal settings.


Y1-Y3 source validation: Rust formatting/parser checks, regression/migration
references and offline build-kit recipe fixtures passed. The snapshot fixture
also requires terminal_flow_control.rs so the packaged agent cannot omit it.
Cargo is mocked in those offline recipe checks. All three new native Linux
regressions and live acceptance remain for the user's build.


## All-application keyboard recurrence — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| Z1 | Reopen the keyboard failure reported in editor and terminal | Live focus/capture reset recovered typing, confirmed by user, without restart/reconnect. Original internal failing stage is unproven. Earlier Ctrl+Q finding applies to terminal pausing only. |
| Z2 | Recover desktop GUI focus across activation and layout changes | Source repaired. Session-global surface ID; reclaim only empty focus in an active viewport; respect local inputs and explicit release. Existing native focus regression strengthened and a new activation/local-field/release regression registered. Native run pending. |
| Z3 | Stop discarding GUI keys when native capture misses them | Source repaired. Forward mapped GUI key edges via fallback, release stale held state, and request asynchronous hook reinstall. Route lock is nonblocking in keyboard callback. Recovery status/logging and failure cleanup added; no text/credential logging. Native/live run pending. |
| Z4 | Preserve host shortcuts and validation | Removed duplicate unconditional Ctrl+Shift+K consumption before the remote-focus gate. Added a GUI key-classification regression that excludes Text/IME replay. Existing Windows build-kit routing picks up the registered tests. No backend replacement or dependency change. |

The currently open rebuilt package was only inspected and used for the safe
unsaved-editor test. These source changes require a further Windows client build
and a new client process before acceptance. No application/native test was
compiled in this review. The remote terminal, EDA files, credentials, connection
lifecycle and host preferences were not changed.


Z1-Z4 source validation: formatting/parser checks, regression/migration source
references and the offline build-kit recipe passed. Cargo was mocked by the
recipe; no application build or native Rust test ran. The two new Windows
regressions and strengthened existing focus regression remain for the user's
build. Live recovery of the old package is evidence of the symptom's recovery,
not live verification of these source repairs.


## AI new-chat controls and model menu — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| AA1 | Add a + new-chat icon immediately left of the AI settings gear | Source implemented. Starts a new chat UUID with an empty transcript/draft, cleared stream/error/restore/attachment/approval state and fresh permission state. Cancels the old turn and replaces its response channel; late old replies cannot mutate the fresh chat. Keeps the selected service, model, workspace and generation preferences. Old chat remains in history. Native/live acceptance pending. |
| AA2 | Correct stretched Custom provider headings in the model popup | Source implemented. Explicit vertical, left-aligned popup layout; provider headings are single-line/truncated with full tooltip rather than justified wrapped text. Popup minimum width is increased within the viewport, and search uses available width. Search and alphabetical lists are retained. Native/live acceptance pending. |
| AA3 | Add useful compact history access | Added a vector history icon left of + where header space permits. History also remains in settings for narrow sidebars. All controls have tooltips; no new raster assets or dependencies. |
| AA4 | Add regression coverage and keep build-kit routing current | Registered two Windows regressions for cancelled/late old events, fresh chat state, separate provider discovery, retained history, and rendered non-justified single-line Custom headings under a justified parent. Native execution remains for the user's build. |

Both the header + and settings New chat button use the same reset action, which
also works while a response or tool approval is pending. In-progress work is
cancelled through the existing cancellation path; completed external actions are
not undone. Model discovery uses its separate existing channel, so replacing a
chat's response channel does not discard provider catalog updates. Every send
gets a new response channel, also isolating late events from earlier turns.

The prior conversation is retained rather than deleted. Global API/provider
configuration and storage validity are preserved; New chat does not bypass
missing credentials, storage failures or tool permission checks. Source-only
verification results are recorded below. No application/native test was compiled
and no API calls or existing personal chat/settings changes were made here.


AA1-AA4 source validation: Rust formatting/parser checks, regression/migration
references and the offline build-kit recipe passed. Cargo was mocked in the
recipe; neither new native regression was compiled or executed. The source
changes require the user's next build for visual and interaction acceptance.

## Compact AI service management, model filters and provider errors — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| AB1 | Constrain the services/API-key window and categorize its contents | Source implemented. Viewport-aware compact window with Services, Models and Add/Edit pages; Connection, Authentication and optional Model groups. Long names/URLs truncate, forms have bounded fields and vertical scrolling. The old oversized window state is discarded; general AI settings are hidden while the manager is open. Native/visual acceptance pending. |
| AB2 | Search models directly in the service manager | Source implemented. Case-insensitive search, alphabetical catalogs, model selection, refresh and exact manual IDs remain in the same window. Adding/editing/removing services keeps the existing per-service identities and vault storage. Native/provider acceptance pending. |
| AB3 | Add an icon-only model filter | Source implemented in both composer model selection and the manager. Funnel icon offers All (default), Free, Paid and Pricing unknown. Filters combine with search and never switch the selected service/model. Pricing is parsed from service metadata, including non-token fees; absent, invalid or dynamic pricing stays unknown. Refresh catalogs to obtain pricing for older saved entries. Free pricing does not guarantee access or available quota. |
| AB4 | Distinguish screenshot capture from new chat | Source implemented. Camera silhouette replaces the attachment plus; new chat retains its plus. Tooltips and accessible labels remain. |
| AB5 | Repair omitted output limits and explain HTTP 402 | Source implemented. A compatible catalog that omits the standard token-limit field can no longer remove the user's configured output cap. Bounded provider errors distinguish reply-budget, general payment and simultaneous-request budget failures; they identify the endpoint host without echoing response bodies, keys or prompts. HTTP 402 is not automatically retried, and services/models are never changed automatically. Actual service credit/access restrictions cannot be bypassed by a client fix. |
| AB6 | Verification and build-kit routing | Five focused native regressions registered for window bounds/search, pricing filters/legacy data, output caps and real loopback HTTP/SSE payment failures without replay or secret disclosure. Native execution and live custom/OpenRouter acceptance remain for the user's build. Source/offline checks recorded below. |

Design reference: VS Code's grouped/searchable settings pattern
(https://code.visualstudio.com/docs/configure/settings), checked against its G2
user reviews (https://www.g2.com/products/visual-studio-code/reviews; 4.7/5 at
review). Reviews also describe settings overload, so this change keeps distinct
small pages rather than adding more controls to one long form. This is a
well-reviewed reference, not a claim that one UI is objectively best.

Provider behavior reference:
https://openrouter.ai/docs/api_reference/errors-and-debugging.
The user's examples were custom APIs (Kimi/Qwen/MiMo) and OpenRouter; no live
provider call, credential lookup, personal-settings mutation or application
build was performed for this change. The precise cause of each real 402 remains
unverified until the rebuilt client receives its provider's response.

AB1-AB6 source validation: Rust formatting/parser checks, native regression
source/reference checks, migration checks and the offline build-kit recipe
passed. Cargo was mocked by the recipe; no native test, compilation or real API
call ran. Model filters expand within the existing picker, avoiding a nested
popup that would close its parent; search/filter clicks keep the picker open.
The added regressions and visual/provider acceptance remain pending the user's
next build. No application package was created or replaced.

## Bound the chat model popup — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| AC1 | Keep chat/settings model menus compact and inside the client window | Source implemented. Both use the same 480-point maximum content width, reduced for narrow windows. Explicit popup layout and content/scroll bounds replace reliance on a default width. Long model IDs truncate with full hover text. Height accounts for space above/below the trigger; search/filter overflow scrolls on short windows. A new popup identity discards the old oversized remembered dimensions. Visual/native acceptance pending. |
| AC2 | Add regression coverage and update build-kit routing | Registered a rendered-popup regression covering a large catalog, long service/model/query text, chat/settings button widths, larger fonts, narrow/wide client windows and placement near the right/bottom edges. No real provider calls or credentials are used. Native execution remains for the user's build. |

Search, price filters and model selection retain their current behavior. No
compiler or application build was run. Formatting, regression references,
migration checks and whitespace checks passed; offline recipe result follows.

AC1-AC2 offline recipe passed with Cargo mocked. The new rendered regression was
registered and source-checked, but was not compiled or executed. Rebuild and open
the model list in chat/settings for visual acceptance; no package was replaced.

## Build duration reporting and optional cache reuse — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| AD1 | Display elapsed time per stage and for the whole build | Implemented. Monotonic stopwatches, stage names/start times, completion/failure durations and a final total including setup/cleanup. Stage worker times are included in existing transcripts. Latest per-stage/status/seconds report is saved under output/reports/build-timing.json. |
| AD2 | Offer faster repeat builds without changing space-saving defaults | Implemented. Build-Faster.cmd and Build-Portable-Faster.cmd are available in both source tools and the existing beginner kit. They opt into the existing cache-reuse flag, preserving Windows compiler/download caches and labelled Linux build layers. Temporary staging is still removed. Normal launchers keep their cache-removal behavior. First runs must fill caches; no measured speedup is claimed. |
| AD3 | Preserve verification and cleanup safeguards | All native regression, package verification, release optimization, hash checks and owned-path cleanup gates remain enabled. No test skipping or broad cache deletion was introduced. Fast mode uses more disk; a later successful normal build reclaims its build caches. |
| AD4 | Verification | Offline fixtures passed for duration formatting beyond 24 hours, running/success/failure report data, and cache-preserving staging cleanup. Existing recipe checks also passed with Cargo mocked. No application build or native compilation was run; full-run timing and actual speed measurements remain for the user's build. |

Kit instructions describe both launchers and the storage tradeoff. The parent's
stage timings include worker startup; worker transcript times cover work inside
that worker, so small differences are expected. Timers print at stage boundaries
and on completion/failure; they are elapsed measurements, not a live ETA.

## AI model-filter click freeze — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| AE1 | Remove the UI deadlock on a physical filter click | Source repaired. The shared filter control now reads response.clicked() before entering ui.data_mut, then passes only the resulting boolean into the state update. Both service-manager and chat/settings pickers use this control. Search, filter selections and saved provider/chat data retain their behavior. Rebuild/restart and live acceptance pending. |
| AE2 | Cover the actual interaction that triggered the freeze | Registered a physical-pointer regression for opening the filter, selecting Free, closing/reopening it repeatedly and drawing subsequent frames at narrow/wide widths. The UI runs on an isolated test thread with a bounded completion wait, so a recurrence fails the build instead of hanging its entire test process. Native compilation/execution remains for the user's build. |

Cause confirmed against the cached egui 0.33.3 source: Response.clicked calls
clicked_by, which reads Context input when a physical CLICKED flag is set.
Context.data_mut holds that same Context's write lock. Calling clicked inside
data_mut therefore re-enters the lock only on the real mouse-click path, matching
the reported freeze. Previous layout-only/filter-matching tests did not exercise
that path. The source audit found no other equivalent Response read inside the
client's data/input/memory mutation closures.

No existing AetherDesk process, provider key, saved setting, chat or remote host
was accessed or changed. No compiler/application build was run. Source/offline
verification results follow below.

AE1-AE2 verification: Rust formatting/parser, migration, regression source/
reference and whitespace checks passed. The full offline build-kit recipe also
passed with Cargo mocked. The physical-click regression is registered for the
user's native test stage; it has not been compiled or executed here. Rebuild and
restart the client before checking the repaired provider/chat filter buttons.

## Screenshot context, AI waiting indicator and remote resolution — 2026-10-16

| Item | Work | Status |
| --- | --- | --- |
| AF1 | Stop treating screenshot base64 as prompt text | Source repaired. Context trimming discounts image URL payload bytes and reserves 4096 estimated vision tokens per image within the existing text heuristic. Whole user/tool turns stay together, and images still consume context. PNG pixels/data are unchanged; the separate request-size limit remains enforced. Actual provider vision tokenization remains authoritative. Native/provider acceptance pending. |
| AF2 | Show progress in the assistant reply area | Source implemented. Animated inline spinner and Working… label appear below the latest message while waiting. They disappear on the first nonempty reply delta and stay gone across checkpoints/tool rounds after visible output. Errors, cancellation, completion and pending user approvals do not show a misleading waiting indicator. New chats/sends reset the state. Native/visual acceptance pending. |
| AF3 | Add icon-only remote resolution access in every presentation mode | Source implemented. A vector resolution icon is available in tab controls and the floating fullscreen/all-monitor dock. Its compact window lists actual virtual pixel sizes at 50/75/100/125/150/200%; unsupported choices are disabled. Tab/current/all use the same validated layout path; physical monitor placement/topology stays unchanged. Dialogs appear in the active viewport and release remote keyboard capture while open. Dock dimensions and its rendered regression include the extra control. Native/host acceptance pending. |
| AF4 | Regression and build-kit coverage | Registered tests for large screenshot payloads without losing complete tool turns; rendered waiting text/first reply/approval/error/cancel transitions; and tab/current/all scaled layouts with unchanged physical bounds. Existing dock rotation/DPI regression exercises the new icon. User build must compile and run these tests; no application/compiler execution occurred here. |

Waiting-indicator reference: Carbon's inline-loading guidance recommends placing
temporary progress where the resulting content appears:
https://www.carbondesignsystem.com/building-blocks/core/components/inline-loading/guidelines.
The provider transport, image encoder, remote video codec/quality and monitor
protocol remain unchanged. Resolution changes use the existing debounced layout
request; they change only that virtual session. If a previously chosen scale is
unsupported when switching monitor mode, that mode falls back to 100% within
the existing protocol limits.

No live API calls, remote-host changes or personal settings were performed.
Source/offline validation results follow; native/provider/display acceptance is
left for the user's build and restart.

AF1-AF4 verification: Rust formatting/parser checks, migration/reference checks,
native regression registration and whitespace checks passed. The full offline
build-kit recipe passed with Cargo mocked. The new native tests were registered
but not compiled/executed. Reply waiting ignores whitespace-only deltas; local
display dialogs retain the existing 100 ms idle repaint cadence so video and
debounced resolution updates continue while keyboard capture is released.
Live screenshot/provider, animation and remote resize/input checks remain after
the user's next build and client restart.

## Connection row alignment and updater discussion — 2026-10-19

| Item | Work | Status |
| --- | --- | --- |
| AG1 | Align active connection names left and statuses right | Source implemented. Each selectable row fills the available space beside the existing disconnect icon. The name is anchored left and status right, with both vertically centered. Long text truncates and the full name/status remain in the tooltip and accessibility label. Native/visual acceptance pending the user's build. |
| AG2 | Discuss automatic updates for installer and portable builds | Discussion only, as requested. Proposed signed versioned full-package releases, a bundled Rust updater helper, optional automatic checking/downloading in Settings, user-controlled restart and rollback. No updater code, setting, publication or packaging change implemented. Current GitHub Windows workflow still builds Flutter and uploads temporary artifacts; a Rust release workflow is a prerequisite. |

AG1 source/offline verification results follow. No compilation, application build,
live connection or personal setting changes are authorized for this task.

AG1 verification: Rust formatting/parser, migration inventory, regression source/
reference and whitespace checks passed. No compiler/native tests or application
build were run. Visual acceptance remains after rebuilding/restarting the client.

## Public automatic updates and frozen build recipes — 2026-10-19

| Item | Work | Status |
| --- | --- | --- |
| AH1 | Settings → Updates and automatic checks | Source implemented: checks on, downloads off, explicit install confirmation, notes/progress, notification, persisted and reverified downloads. Public downloads require no GitHub account or shared credential. Native/visual acceptance pending. |
| AH2 | Signed installer and portable updating | Source implemented: embedded Ed25519 key, expiry/version/target/path/size/hash checks; isolated helper DLLs; active-work gating; existing Inno installer identity; portable backup/journal and first-launch recovery. Installer failure logs retained. Full installer/portable and interrupted-upgrade acceptance pending; not claimed power-loss-proof. |
| AH3 | Publisher identity and public distribution | Configured: local signing key protected in Windows Credential Manager and encrypted Actions secret in private AetherDesk-Rust. Public ashkanr65/AetherDesk created with download-only README. Former project retained as private archived AetherDesk-Legacy-Private-20261019. No source or binary published publicly. |
| AH4 | Release tooling and private source push | Manual Rust release workflow and publisher-only signing/verification/draft-upload tools implemented. Both application workspace locks retained for reproducible builds. Source pushed to private main; no release build/workflow triggered. Authenticode certificate and live acceptance remain outstanding. |
| AH5 | Repair source-change failure during packaging | Stages 2–6 now run their recipe from the verified stage-1 snapshot; checkout edits enter the next build. Snapshot tampering and mixed initial snapshots still fail. Offline fixture passed for live edits versus snapshot changes. The old step-4 log was a concurrent-edit guard, not failed X11; its X11 regression passed. |

The user authorized GitHub setup/public downloads and source push. They retained
all application compilation/building. No native compiler or application build
has been run here. See UPDATES.md for release/bootstrap/recovery requirements.

AH verification: all native UI Rust files passed formatting/parser checks;
migration/reference and registered-regression source checks passed; the full
offline recipe passed with Cargo mocked, including frozen-snapshot edit detection.
Three publisher fixtures passed, covering signed inventory/hash verification,
tampered packages and the production PowerShell extractor's safe/unsafe ZIP cases.
The manual-only workflow parsed successfully and the changed-source credential
scan found no key material. Native compiler, actual installer/portable upgrades,
multi-instance locking and restart recovery remain pending. No binary is published.

The user's later instruction requires all source pushes on main. The temporary
local review branch was never pushed; its commits were transferred to main and
the local branch removed. Legacy Flutter/agent workflows are now manual-only, as
is the new Rust release workflow, so this main push will not start compilation.

Private main now contains the complete pending Rust source plus updater work.
The public repository contains only its download README, with no released binary
or application source. Initial update checks treat an existing public repository
with no published releases as no update, rather than an authentication failure.

First-launch review also preserves the helper's plan/journal/backup until health
acknowledgement; cleanup does not retire a stage merely because the new version
has already started. A dedicated filesystem regression is registered for this
case and remains awaiting the user's compiler/native test stage.

## Initial release preparation — 2026-10-19

| Item | Work | Status |
| --- | --- | --- |
| AI1 | Verify the user's completed build | Passed: all seven stages, 39:55 total. Windows/Linux compiler results and bundled X11 passed. All four new updater regressions passed. Built source fingerprint 1f136a5e4d3b026bc4e03c7e8b1445be183fd52e15a0b4ab03c4eb2fe06fd7ce matches the current runtime source. |
| AI2 | Check the portable distributable | Passed: all 7,158 extracted files match the verified package. Existing executable passed isolated dark/light rendering and popup-editor lifecycle checks without SDK PATH. No application compilation, provider call or host connection performed here. |
| AI3 | Publish signed preview assets | Complete: user-approved 0.1.2 build 1 preview published at https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build1 on 2026-10-19. Installer, flat portable ZIP and signed metadata uploaded; all three GitHub SHA-256 digests match local assets. Anonymous access verified. Source remains private; preview excluded from stable automatic updates. |
| AI4 | Stable updater/release acceptance | Remaining: real two-version upgrade, current installer upgrade/recovery, and broader live R12 checks. Windows Authenticode signing remains unconfigured. Build success is not a claim of full replacement equivalence. |

Publisher preparation found 32 dependency licence files with reproducible dates
before ZIP's 1980 minimum. The publisher now clamps ZIP timestamps, with a fixture
covering a 1970 file and unchanged file hashes. This publisher-only adjustment does
not alter the built runtime. The verified application remains the e9596cf source
snapshot identified in AI1; no application rebuild is needed for this metadata fix.

## Update acceptance candidate — 2026-10-19

| Item | Work | Status |
| --- | --- | --- |
| AJ1 | Prepare a newer release for the user's upgrade test | Complete: release.json is 0.1.2 build 2; release notes distinguish prior build 1 evidence from pending build 2 validation. No application code changed. |
| AJ2 | Build and verify build 2 | Passed: all seven user-run stages in 1:08:28; built source fingerprint 47984201d8a52be22bc85f1dc16b555b60fd07f29b415c7a4ea928172d055972 matched commit 4b9c3a4. Windows/Linux regression reports and bundled X11 passed. Existing executable passed isolated dark/light/editor checks. Signed installer/flat portable ZIP/metadata prepared and verified; all 7,158 ZIP entries match the signed inventory. All three uploaded GitHub asset SHA-256 digests and lengths match local files. Draft release 417338992 ready for review; source private. No compilation performed by the assistant. |
| AJ3 | Publish build 2 and test real update detection | Passed: user approved normal publication; v0.1.2-build2 published publicly on 2026-10-19. Anonymous latest-release feed returns build 2. Reopened the user's existing build 1 (it had been closed), clicked Settings → Updates → Check now and observed the available update, build 2 notes and Download update button. Source remains private. |
| AJ4 | Test download, replacement, restart and preservation | Remaining: no download, installation or restart performed. Updates panel left open in build 1. Real portable/installer upgrades, recovery and preservation of settings/credentials/chats still need acceptance. |

Build 2 release: https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build2
Signed files: AetherDesk-Rust-output/releases/0.1.2-build2-20261019-153901.
Acceptance evidence: AetherDesk-Rust-output/LATEST-RELEASE-CHECK.txt points to
the isolated UI check, signed ZIP inventory and live-update-detection.json reports.
Build 1 is running from the user's Documents folder. Build 2 is now public and
available to eligible stable-channel users; nothing was installed during this
detection test, and no connection was interrupted.

## Independent release documentation — 2026-10-19

| Item | Work | Status |
| --- | --- | --- |
| AK1 | Document the complete build/sign/publish/test process | Complete: RELEASE-GUIDE.html in the beginner kit, with a versioned source copy at the repository root. Twelve sections cover publisher setup, versions, cache choices, build/package provenance, signing, ZIP inventory, draft/upload verification, preview/normal publication, actual upgrade tests and recovery. Eleven PowerShell examples parsed without execution; internal links and matching copies checked. No key material included. |
| AK2 | Make the guide discoverable | Complete: beginner-kit START-HERE.md and source build-kit README link the offline guide. It includes copy buttons and print/save-as-PDF support. Built-in browser local-file preview was blocked by its URL policy; no rendered-layout check is claimed. |
