# AetherDesk 0.1.2 build 8 - Window stability and consistent indicators

- Use the same information-colour Ready dot on all four home cards in both themes.
- Share monitor-aware sizing across the main window, Files, Editor, file comparisons and language-edit reviews. Wait until dragging and DPI changes settle before fitting the destination work area; preserve the normal window size and Windows snap/maximize behavior.
- Reflow narrow Files windows into local/server views, keep navigation controls within each pane and preserve the connection and browsing state.
- Use Direct3D 12 for Automatic rendering on Windows to avoid the Intel OpenGL path involved in the reported native crash. Display preferences retain an OpenGL compatibility option; renderer changes require a restart.
- Add graphics and geometry diagnostics and best-effort crash metadata without collecting chat, terminal or process-memory contents.
- Expand resize, dirty-editor and isolated native popup lifecycle regression coverage.

Prepared source changes for build 8. Compilation, native tests and packaged
mixed-DPI/graphics acceptance remain pending. The published auto-update remains
build 7; no new release has been uploaded. The recorded crash was in Intel's
OpenGL driver; these mitigations still require verification on the affected PC.
