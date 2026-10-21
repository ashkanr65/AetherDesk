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

## Preferences and public product presentation — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AL1 | Stable Log files popup | Implemented: fixed screen-bounded dialog, bounded scroll viewport and nonwrapping virtualized rows. Scroll regression passed in the user-run build 3 tests. |
| AL2 | Categorized application Preferences | Implemented: General, Display, Connections, Files and Editor, AI Assistant, Keyboard, Updates and Logs. Controls are wired to startup/disconnected restoration, quit confirmation, connection defaults/timeouts, editor/directory defaults, custom navigation, log limits and existing AI/update panels. |
| AL3 | Independent theme customization | Implemented: 30 roles each for light/dark, sample GUI previews, colour pickers/hex values, contrast warnings, apply/cancel/reset and palette-only import/export. Missing/invalid preferences preserve compatibility and damaged files. |
| AL4 | Public-facing AetherDesk naming | Implemented: Preferences/Log files labels, About version/build/licence/project information, clean package/installer/shortcut names and product README. Historical verification remains developer-only. Existing AppId, install directory, credential targets, data paths and update key preserved. |
| AL5 | Validate and build the next candidate | Build 0.1.2 build 3 prepared. Seven native regressions registered and inventoried. Rust formatting/parser, catalog validation, offline recipe/publisher safeguards and two signature/package fixtures passed without compiling. Eleven guide examples parse; guide copies match. User build 3 compilation/native tests and isolated packaged rendering/editor verification subsequently passed; see AN1. Broader live/upgrade acceptance remains pending. |
| AL6 | Publish source, licences and combined release repository | Complete: full source history merged into public ashkanr65/AetherDesk main (4a7b7dc); anonymous source/licence access verified. Existing build 1/2 tags unchanged; latest build 2 and all three asset digests unchanged. Existing signing key configured as the canonical repository's encrypted Actions secret. Old private source renamed AetherDesk-Source-Archive-20261020 and archived. Publisher/workflow/release guide now use the single public repository and exact main commit. |
| AL7 | Complete new-package and real upgrade acceptance | User build 3, all seven new native tests and isolated packaged dark/light/editor checks passed. Remaining: manual Preferences interactions, broader connection checks and real update download/replacement/restart/settings and credential preservation. Build 3 stable publication is recorded under AN3. |

## Build 3 compiler compatibility repair — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AM1 | Correct the icon-button pressed-state API | Fixed: use Response::is_pointer_button_down_on(), confirmed in the cached pinned egui 0.33.3 source. Keeps the pressed/hover icon colours and normal custom icon colour. |
| AM2 | Remove reported stroke-width inference warnings | Fixed: explicit f32 widths at all ten reported calls in navigation, Preferences and theme styling. Values and appearance are unchanged. |
| AM3 | Verify and rebuild | Rust parser/format check and source diff checks passed. The user's subsequent build passed compilation and native regressions; see AN1 for evidence and AN3 for publication. |

## Build 3 stable update publication — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AN1 | Verify completed user build | Passed: all seven stages in 00:26:29; package fingerprint d7e528b21e02636315f09631031974a09bea416e58cf40eb059d9e0cc4a097c7 matches current source inputs. Windows: 272 registered / 246 required; Linux: 95 registered / 80 required; compiler and bundled X11 evidence pass. Licence files match. Isolated packaged dark/light/editor-popup checks passed under acceptance/r12-20261020-114717. |
| AN2 | Sign and verify update assets | Passed: existing protected key signs 0.1.2 build 3. All 7,158 ZIP entries match the signed inventory; installer matches build output. All three uploaded asset lengths and SHA-256 digests match local verified files. Release tag targets e17683b6a4952dc4cc09f76c38f653f69ff95899; application/build inputs match d24793c and the recorded source fingerprint. Evidence: output/acceptance/update-build3-20261020; LATEST-SIGNED-RELEASE.txt and LATEST-RELEASE-CHECK.txt updated. |
| AN3 | Make build 3 available to auto-update | Complete: user-authorized normal/latest v0.1.2-build3 published on 2026-10-20, release 418243553. Anonymous latest feed and the exact runtime metadata API route return build 3; signature verifies with the existing pinned key and all asset hashes match. https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build3 . Real installation/restart/data-preservation acceptance remains separate; no user installation was changed. |

## Preferences layout repair — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AO1 | Stack category content vertically | Implemented: explicit top-down content layout isolates the form from the horizontal navigation/container. All eight category titles start their own column; options follow below. Existing window sizing and left navigation are preserved. |
| AO2 | Align controls and use consistent text | Implemented: body-sized labels, descriptions, values and buttons; shared field grids; aligned palette picker/hex rows; wrapped descriptions and vertical scrolling bounded to the content column. Preview drawing clips to its sample area. |
| AO3 | Verify and prepare the next build | Build 0.1.2 build 4 reserved because build 3 is already published. Two rendered category/font/row geometry regressions registered for Windows. Rust parser/format, catalog mapping and offline recipe checks passed (Cargo/network/signing mocked in recipe fixtures); The user's subsequent build 4 passed compilation and both layout regressions; isolated packaged rendering/editor checks and stable publication are recorded under AP1–AP3. |

## Build 4 stable update preparation — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AP1 | Verify user build 4 | Passed: all seven stages in 00:52:06. Source fingerprint 8acf6660e3f333fb5523a15be2149f010f5ade12dae2494a63e7ed47b578766a matches current inputs. Windows 274 registered / 248 required and Linux 95 / 80 pass, including both new rendered Preferences tests. Bundled X11 and licence checks pass. Isolated dark/light/editor check: acceptance/r12-20261020-130828. |
| AP2 | Sign, upload and enable stable update | Passed: existing protected key signs build 4. All 7,158 ZIP entries match the signed inventory; installer matches build output and version 0.1.2.4. All three uploaded asset SHA-256 digests and lengths match local verified files. Tag targets d76955bc94c5bca25e20693d6421f8250a1002b0; application inputs match 2c95b0f and the recorded fingerprint. Evidence: output/acceptance/update-build4-20261020; latest signed-release and check pointers updated. |
| AP3 | Enable and verify stable auto-update | Complete: user-authorized v0.1.2-build4 published as normal/latest on 2026-10-20, release 418307323. Anonymous latest feed and the exact runtime metadata route return build 4; downloaded signature verifies with the established pinned key. https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build4 . Real upgrade/restart/data-preservation acceptance remains separate; no user installation was changed. |

## Portable update failure and Preferences sliders — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AQ1 | Diagnose the actual restart/recovery failure | Confirmed: the build 3 to 4 update left a build 4 executable with build 3 files after overwrite and rollback both encountered read-only bundled Linux runtime files. Found 58 read-only package files; reproduced Windows overwrite denial with disposable files. The preserved recovery stage contains the signed download, complete extraction, previous files and journal. |
| AQ2 | Recover the user's portable copy | Complete: validated the existing Ed25519 signature and all 7,158 source files, repaired signed-owned files under an exclusive installation lock, normalized owned read-only attributes and preserved unknown files/recovery backups. Current inventory fully matches build 4; pending reference is clear and highest-version floor remains 0.1.2 build 4. Renamed only the user's portable folder to Documents/AetherDesk, per the corrected request. Isolated dark/light/editor self-check passed; reopened normal window reports AetherDesk and Responding=true. Evidence: output/acceptance/portable-recovery-20261020-133017/recovery.json and ui/ui-smoke.json; earlier partial-file backups are retained in sibling portable-recovery folders. |
| AQ3 | Fix replacement, rollback and restart behavior | Implemented for build 5: flushed sibling temporary files, owned-file read-only handling, bounded Windows replacement retries, atomic metadata publication, complete-journal validation, best-effort restoration of every entry and concrete error paths. Safe extraction/replacement failures reopen the prior application when files are intact/restored. Three Windows filesystem regressions are registered as required build checks. |
| AQ4 | Show slider axes in Preferences | Implemented: Application scale and Application font size have a six-point horizontal rail, visible track colour and filled section in both themes, with aligned numeric values. |
| AQ5 | Validate and publish build 5 | Rust parser/format and diff checks pass. Offline recipe, publisher/extractor fixtures and catalog safeguards pass with Cargo/network/signing mocked as appropriate. No compilation performed. Remaining: user's build 5, the three native regressions, real automatic replacement/restart/settings and credential preservation, installer upgrade and clean-machine checks. Build 5 is reserved and is not published. |

## Window snapping, monitor changes and narrow layouts — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AR1 | Restore Windows snapping on this PC | Complete: the user's Windows Snap windows preference was disabled. After explicit approval, enabled it using SystemParametersInfoW and verified SPI_GETWINARRANGING changed from false to true. Evidence: output/acceptance/window-layout-20261020/windows-snapping.json. This is a user-approved PC setting, not an automatic setting change shipped by the application. Windows+Shift+Left/Right moves a window between monitors; Windows+Arrow snaps it. Live physical-shortcut acceptance remains for the user. |
| AR2 | Handle monitor resolution and DPI changes | Implemented: root window starts with monitor clamping and a smaller minimum; monitor/DPI/zoom changes refresh the minimum so half-screen snapping fits. Oversized normal windows refit once, without moving monitor origins, overriding chosen fonts/zoom, or fighting manual resizing/maximized/fullscreen windows. Remote framebuffer and monitor resolution are unchanged. |
| AR3 | Reflow the application instead of overlapping text | Implemented: welcome text/card heights are measured and cards switch between one/two columns; header controls reserve their own space, status text truncates with full details on hover, and the navigation rail scrolls when short. Sidebar width preference survives shrinking and is restored when space returns; a drawer handles very narrow windows. Preferences switches to compact category navigation, while Preferences/connection/terminal forms retain bounded scrollable controls and wrapping actions. |
| AR4 | Verify the responsive layout changes | Five new required Windows regressions registered: two viewport/DPI policies, welcome text geometry across both themes/large fonts, sidebar preference restoration, and compact Preferences across all eight categories. Rust parser/format, registry and offline recipe/publisher safeguards pass (Cargo/network/signing mocked in recipe fixtures). No compilation performed. Remaining: user build 5, native tests, moving/snapping the built application across real monitors and checking both themes at narrow widths. Build 5 also contains the earlier updater fix; it is not published yet. |

## Welcome shortcut row regression repair — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AS1 | Fix the reported narrow welcome action overlap | Implemented: measure both the New Connection button and shortcut hint before choosing a horizontal row or a vertical stack. Removes the wrapping-horizontal-row interaction that painted shortcut text over the button at width 280/font 13. Keeps the existing geometry regression unchanged. |
| AS2 | Start the requested full build | Source parser/format and patch checks pass. The user explicitly authorized starting Build-Everything this time and requested not to wait for results. Build launch and completion status are recorded separately below; no successful result is assumed. |

## Build 5 update preparation — 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AT1 | Verify the completed build | Passed: all seven stages in 00:39:06. Windows 282 registered / 256 required and Linux 95 / 80 pass; all eight new updater/layout regressions, including the formerly failing welcome shortcut row, pass. Packaged X11 passes. Source fingerprint a89aab67d0600e4fad82747126ac182c74498100ee57aa80a4a0647249397c38 matches the current application inputs from a830fb6. Isolated packaged dark/light/editor check passed without SDK PATH. Evidence: output/acceptance/update-build5-20261020. |
| AT2 | Sign and publish the stable update | Complete: v0.1.2-build5 is published as the latest normal release (418397801), at https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build5 . Existing protected key signs build 5. All 7,158 ZIP entries match signed hashes; installer matches build output and file version 0.1.2.5. All three uploaded asset lengths and GitHub SHA-256 digests match local verified files. Anonymous latest feed and exact runtime metadata route return build 5; downloaded signature verifies with the pinned key. Tag targets afa97c93f53f2b1859c1a8d1eaf17f695ddc81e5; application inputs match a830fb6 and the verified fingerprint. Evidence: output/acceptance/update-build5-20261020; signed release and latest pointers updated. No user installation was modified during publication. |
| AT3 | Real upgrade and monitor acceptance | Remaining: real two-version automatic replacement/restart and user-data preservation, real monitor movement/snapping, installer upgrade and clean-machine acceptance. Earlier clients with read-only package files may need repair/manual upgrade; the user's previously repaired portable build 4 has normalized package attributes. |

## Compact welcome, section Preferences and public release candidate - 2026-10-20

| Item | Work | Status |
| --- | --- | --- |
| AU1 | Remove large welcome-page gaps | Implemented for build 6: content-sized action rows replace the remaining-height layout; compact vertical spacing and a two-column card threshold keep ordinary desktop viewports together. Measured button/shortcut width handling and tiny-window scrolling fallback remain. New geometry regression checks all welcome text and four cards at real desktop heights, not only an artificially tall viewport. |
| AU2 | Fix Preferences scrolling and Display overlay | Implemented: allocate the window bounds once; fixed left navigation (category selector when narrow), independently clipped content scroll area, fixed status line and section action footer. Content and preview painting cannot cover the action area or enlarge the window. Added a regression that actually scrolls Display to its bottom and checks footer/navigation stability, clipping and reachable import controls. |
| AU3 | Independent Apply, Reset and Cancel | Implemented: only the selected category's fields are applied, reset or discarded. Applying keeps Preferences open and preserves other categories' drafts; invalid Display colour text cannot block General. Closing the title bar discards remaining drafts. AI and Updates retain their own management dialogs; their launcher categories disable Apply/Reset. Persistence/reset/cancel isolation regression added. |
| AU4 | Prepare first public-facing release description | Prepared: build 6 reserved, comprehensive RELEASE-NOTES.md covers connections, files/editing, AI, appearance, installation, updates, protected data, licensing, feedback and material limits. Release notes describe the product for new users; they do not claim unperformed live acceptance. |
| AU5 | Validate and build candidate | Complete: user's build 6 passed all seven stages in 00:52:30. Windows 285 registered / 259 required; desktop 163 passed, 5 intentionally ignored; Linux 95 / 80 and packaged X11 pass. All three new native layout/section-isolation tests pass, including real scroll geometry. Package fingerprint eff8a47538de60a5b944bd1b4a4b3b89571ee5c2ad292a96e6187e3ef4393a79 matches current source at 88501d2. Completed executable passed isolated dark/light rendering and editor lifecycle checks in 1.342 seconds; both home screenshots visually confirm four cards fit without scrolling. Evidence: output/acceptance/update-build6-20261020. |
| AU6 | Publish and retire old GitHub releases | Complete: v0.1.2-build6 published as latest stable on 2026-10-20 at 14:21:38 UTC (release 418487284), with comprehensive product release notes. Existing protected Ed25519 key signs the package; all 7,158 archive file hashes, installer bytes/version 0.1.2.6 and three uploaded asset hashes/lengths verified. Anonymous latest feed and exact runtime metadata route return build 6; downloaded signature verifies with the pinned key. Then removed the four remaining older GitHub releases (builds 2-5) and their assets as authorized. Build 6 is now the only release; source history/tags, local release evidence and recovery backups are preserved. Public feed verified again after cleanup. Tag targets 88501d29782509af2165dd4252c3bf9c7af560a6. https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build6 . |

## Build 6 publication acceptance limits - 2026-10-20

The public update feed, pinned signature, uploaded assets and packaged home
rendering are verified. No normal user installation, saved profiles, credentials
or chats were modified during this publication. A real automatic
replacement/restart with preserved user data, installer upgrade and clean-machine
acceptance remain separate pending checks; build and feed verification do not
claim those checks passed. Local release and old-recovery evidence are retained.

## Remote mouse mappings and wheel direction - 2026-10-21

| Item | Work | Status |
| --- | --- | --- |
| AV1 | Diagnose without changes | Complete: found X11-style client button IDs conflicting with host evdev-style IDs, unchanged Windows vertical deltas entering positive-down Wayland axes, inconsistent native/GUI horizontal signs and rejected side-button IDs. Diagnosis was read-only. |
| AV2 | Repair both client routes and the host | Implemented for build 7: shared documented pointer contract (1 left, 2 right, 3 middle, 4 Back, 5 Forward); common client conversion helpers; shared host evdev mapping including legacy side IDs 8/9; right-positive/up-positive wheel contract converted once to Wayland. Continuous and v120 directions agree and wheel source is identified. Unknown button IDs no longer stop the Wayland capture loop. Keyboard, framebuffer and colour processing are unchanged. |
| AV3 | Add meaningful regression coverage | Added three required native checks for native/GUI parity, full wire/evdev mappings and Wayland axis frames. Extended the existing real X11 renderer/input fixture to check all buttons, press/release edges, legacy side IDs, both wheel axes/directions and capture surviving an unsupported button. Registered the extended fixture under desktop input coverage. |
| AV4 | Verify and push source | Source parser/format and whitespace checks passed across the UI, protocol, Wayland agent and evdev backend. Catalog validation, two offline registry fixtures and the full offline recipe/publisher safeguards passed (Cargo/network/signing mocked in those fixtures). Source committed as b46adb1 and pushed directly to public main. No compilation, new native test execution or package build performed for this step. |
| AV5 | Build and verify before auto-update | Build verification passed: all seven stages in 00:39:55; Windows 287 registered / 261 required (desktop 164 passed, 5 intentionally ignored), Linux 96 / 81 and packaged X11 pass. All three new mouse regressions and the extended real X11 button/wheel fixture pass. Package/source fingerprint matches 9d2c352c7751cd099e0cfbbd31226b848abf059ab400b0ce7b9c2eb876956f3b. Signing, upload and public stable-feed verification are complete; see AW2. Manual packaged mouse/scroll/EDA zoom checks in tab/fullscreen/all-monitor modes and real automatic replacement/restart acceptance remain separate. Reconnect with the updated client/agent together for the complete fix. |

## Build 7 stable update publication - 2026-10-21

| Item | Work | Status |
| --- | --- | --- |
| AW1 | Verify completed user build | Passed: all seven stages in 00:39:55. Package fingerprint 9d2c352c7751cd099e0cfbbd31226b848abf059ab400b0ce7b9c2eb876956f3b matches current application inputs from 0202d0c. Windows 287 registered / 261 required; desktop 164 passed with 5 intentionally ignored; Linux 96 / 81 and packaged X11 pass. All three new mouse regressions and the real X11 button/wheel fixture pass. Isolated completed-executable dark/light/editor lifecycle check passed in 1.826 seconds. Installer file version is 0.1.2.7. Evidence: output/acceptance/update-build7-20261021. |
| AW2 | Sign and enable stable auto-update | Complete: v0.1.2-build7 published as normal/latest on 2026-10-21 at 07:04:33 UTC (release 419249185), https://github.com/ashkanr65/AetherDesk/releases/tag/v0.1.2-build7 . Existing protected Ed25519 key retained. All 7,158 portable archive entries match signed hashes; installer matches build output. All three uploaded asset lengths and SHA-256 digests match local verified files. Anonymous latest feed and exact runtime metadata API return build 7; downloaded signature verifies with the pinned key. Tag targets 6132def94649b90f88214618659d912e4db99ca0 (release documentation only after tested application source). Latest signed-release and verification pointers updated. No normal user installation, saved profiles, credentials or chats modified. |
| AW3 | Remaining live acceptance | Pending: physical mouse/EDA zoom behaviour in the packaged client across tab/fullscreen/all-monitor modes, real automatic replacement/restart and preserved user data, installer upgrade and clean-machine acceptance. The source and real X11 regression results do not claim these manual checks passed. Start an updated remote session so the host-side wheel fix is used. |

## Consistent home-page Ready dots - 2026-10-21

| Item | Work | Status |
| --- | --- | --- |
| AX1 | Match SSH Terminal's Ready indicator to the other cards | Implemented: all four home cards use the theme's information colour. Removed the special-case green SSH dot; these static labels do not indicate live connection status. Build 8 reserved because build 7 is already published. |
| AX2 | Verify and deliver | Source formatting/parser and whitespace checks pass. Task list and candidate release notes updated. Compilation and packaged visual verification remain for the user's next build; no new release published. |

## Shared native-window sizing and crash mitigation - build 8 (2026-10-21)

| Item | Work | Status |
| --- | --- | --- |
| AY1 | Diagnose the last SSH+X11 crash and mixed-DPI drag | Complete: last session PID 920 reconnected at 08:30:11 UTC; Windows recorded access violation 0xc0000005 and callback exception 0xc000041d in Intel igxe2lpgicd64.dll 32.0.101.8860 at 08:30:28/33 UTC. No dump or resize trace survived, so the exact triggering stack is not established. Monitors have 200%/100% scaling and different work areas, with the secondary at a negative X origin. Source showed immediate root-only correction during DPI changes and unbounded two-pane Files layout. |
| AY2 | Share sizing across main, Files, Editor and reviews | Implemented: read actual Windows work areas/decorations; defer corrective sizes while native move/resize is active, DPI disagrees, or geometry is settling. Preserve the user's normal size independently of application zoom and restore it on a larger work area. Keep maximized/fullscreen/snap geometry under Windows control. Stable per-viewport initial builders and separate sizing state cover Files, Editor, comparison and language-review windows. |
| AY3 | Reflow Files safely | Implemented: bounded/clipped pane areas; local/server selectors below the two-pane width threshold; full-width paths and wrapping navigation buttons. Hidden panes cannot accept a drop. Browsing state and the connection are retained across layout changes. Editor controls are bounded and scroll within their area on short/narrow windows; review actions stay outside the content scroll. Editor dirty/save/recovery behavior is preserved. |
| AY4 | Avoid the faulting OpenGL route and improve evidence | Implemented: Automatic uses Direct3D 12 on Windows, with an explicit OpenGL compatibility option in Display preferences (restart required) and AETHERDESK_RENDERER=opengl startup override. Non-Windows retains OpenGL. Remote codecs, resolution and colour data are unchanged. Bounded geometry-transition and graphics-adapter logging plus best-effort synchronous panic/native exception metadata reports in the existing logs folder; no memory dumps or chat/terminal payloads. Windows Error Reporting remains enabled. |
| AY5 | Regression coverage and delivery | Added required checks for settled monitor transitions, active drag/incoherent DPI suppression, independent popup state, actual narrow Files painting, renderer selection and crash-record contents; expanded dirty-editor layout coverage. Extended the isolated packaged UI self-check to resize, minimize, restore and close real Files and Editor windows, then verify the saved fixture. Source parser/format and whitespace checks, catalog validation, two offline registry fixtures and the full offline recipe/publisher safeguards pass (Cargo/network/signing mocked). Compilation and all new native checks remain pending for the user's build. Release identity remains 0.1.2 build 8; nothing published. |
