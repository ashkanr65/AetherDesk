AetherDesk 0.1.2 build 4 — Preferences layout

- Keep the Preferences window size and left category navigation.
- Place each category title at the top with options stacked vertically below it.
- Use the application's body text size consistently for descriptions, labels, inputs and buttons inside the content column.
- Align labels, values, shortcut controls and palette colour/hex controls in rows; wrap long descriptions and scroll vertically inside the window.

Validation status:

- Added two rendered layout regressions covering all eight categories, normal/large fonts and aligned field rows. Registered in the build kit.
- Source/parser and offline checks do not replace native compilation or packaged UI verification. The user's next build must run these checks before publication.
- Build 3 remains the published stable release. This candidate has not been built or published; no user installation was changed.
