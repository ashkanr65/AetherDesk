# Contributing to AetherDesk

Describe the user-visible problem, connection mode, version/build, package type
and a small reproduction before proposing a change. See SECURITY.md for private
security reports. Remove host addresses, usernames, licence-server details and
credentials from public examples unless they are deliberately fictitious.

The native application is in `ui/rust`; shared services and Linux agents are in
`core/rust`. Frozen migration references are historical verification inputs, not
the application to modify. Read the build-kit instructions before building.

Preserve both Cargo.lock files. Update dependencies explicitly and review the
resulting lock changes; do not regenerate them merely to bypass a locked build
failure. Run formatting and regressions appropriate to the affected component.
Register meaningful new behavior tests in rust-regressions.json under the
existing feature, or update both the inventory and catalog for a new feature.

Keep input release, tab isolation, host-key checks, original file revisions,
bounded buffers and update verification intact. Do not substitute lossy image
conversion for a responsiveness fix where exact application colours matter.

Use isolated fixture accounts and disposable files for integration testing.
Never automate edits in somebody's live EDA library or close their unsaved
application work. Include what was actually tested and what still needs a real
application, mixed-DPI, installer or upgrade check.

Automatic checks do not publish. The complete manual build, package checks,
signed update verification and real application acceptance remain required for
a release. Do not describe compiler-test success as complete live acceptance.
