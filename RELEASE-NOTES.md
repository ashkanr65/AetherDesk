# AetherDesk 0.1.3 - build 15

- Fix missed clicks and drag releases when switching windows in Remote Applications. Host dialogs open in their own views, and opening another view keeps the pointer in place.
- Update the bundled Xwayland so mouse clicks in Remote Applications reach the target window after the pointer moves.
- Keep Virtuoso tooltips and other Remote Applications popups at their intended size so Windows window recovery cannot stretch them or cause flicker.
- Create new Linux sessions folders with owner-only permissions (mode `0700`). When the connecting user owns an existing sessions folder and has write and search access, AetherDesk automatically removes group and other write permissions before opening the session. Existing sessions can keep running. If the folder needs an administrator’s attention, AetherDesk shows the issue and how to address it.
- Show root filesystem usage in the compact status bar and Monitoring alongside the home filesystem when they are separate mounts.

[Earlier release notes](docs/release-history.md).
