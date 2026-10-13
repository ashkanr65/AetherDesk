# Rust conversion status

Clone: C:\Users\rezae\Documents\Temp\Aether\AetherDesk-Rust.
Original: C:\Users\rezae\Documents\Temp\Aether\AetherDesk (unchanged).
Baseline commit: d000a918024618c0d5de519fb08b1ccbf37cc784.

## Work implemented

- Separate native Rust workspace and native desktop development preview.
- Existing saved-session JSON schema, enum fallbacks, username consent and
  master-password flags ported into a Rust model.
- Independent session/tab state, sidebar selection preserving the workspace,
  and pointer-anchored resize calculations.
- All existing physical-key scan codes and lock modifiers copied exactly.
- Legacy Linux control/Alt key encodings.
- Existing five-colour palette in the native preview.
- Read-only import of saved profiles, searchable profile/favorites list,
  independent preview tabs and light/dark switching.
- Clone build outputs isolated in AetherDesk-Rust-output.

This does not yet establish behavioral parity. The preview opens no network
connections and is explicitly named aetherdesk-rust-preview.

## Work remaining

| Area | Required result |
|---|---|
| Connections | Native UI around existing SSH/auth/PTY/known-host behavior; event-based output, no UI-thread blocking |
| Terminal | Full VT/ANSI parser and GPU text renderer, selection, clipboard, scrollback, themes, shell shortcuts |
| SFTP | Both panes, editable paths, all menus, transfers, resume/concurrency, remote editing/local application handling |
| Credentials | Exact Windows Credential Manager + DPAPI/master-password behavior and existing key namespaces |
| Editor | Separate native popup; dirty-close/conflict handling, languages/LSP, completion, search/diff, file saves |
| Presentation | Rust port of native Win32 fullscreen/multi-monitor/docking/clipboard/input/decoder integration |
| Host lifecycle | Same retained desktop IDs, reconnect semantics, end-session confirmation and host-process cleanup |
| X11 | Bundled authenticated X server management, session isolation, cleanup |
| AI | Rust provider/stream/tool loop, matching history/config, all provider/model options and approval scopes |
| Tools | Existing manifests, revisions, install/run approval and Windows/SSH scripts |
| Logging | Exact redaction, rotation, Application/Host/Audit/Problems UI, export/filter/follow |
| Packaging | Rust client executable, installer and portable ZIP, offline dependency bundles, existing release numbering |
| Verification | Port behavior tests, original/Rust comparison, live Windows+SSH+desktop+X11 checks and performance measurements |

The structured feature gate is rust-migration.json. Its replacementReady value
is false. Every field marked partial or pending must be implemented and tested;
ported_unverified is not a passing release state. Existing Dart/C++/Python
sources remain as references until equivalent functionality is verified.
Third-party OS/codecs/X server binaries and user scripts remain supported.

## Checks performed

Rust formatting/parser checks and build-output path regression tests passed.
No Cargo compilation, Rust unit-test execution, app build, installer or ZIP was
performed. A dependency lockfile, compile checks and native UI verification are
still required. No performance improvement is claimed without measurements.

The preview uses eframe 0.33.3:
https://docs.rs/eframe/0.33.3/eframe/trait.App.html

## Manual preview commands

See ui/rust/README.md. The normal beginner kit still targets the reference
application until the Rust packaging port is complete.

Run ui/rust/Check-Migration.ps1 for reference/hash/key-map checks.
Its -Release switch must reject this incomplete migration.
