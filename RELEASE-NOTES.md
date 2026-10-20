AetherDesk 0.1.2 build 4 — Preferences layout

- Keep the Preferences window size and left category navigation.
- Place each category title at the top with options stacked vertically below it.
- Use the application's body text size consistently for descriptions, labels, inputs and buttons inside the content column.
- Align labels, values, shortcut controls and palette colour/hex controls in rows; wrap long descriptions and scroll vertically inside the window.

Validation status:

- The user-built installer and portable package passed all seven stages in 52 minutes 6 seconds on 20 October 2026.
- Windows/Linux regression checks and packaged X11 passed, including the two rendered Preferences layout tests covering all eight categories, normal/large fonts and aligned field rows.
- Built package inputs match the current source fingerprint. Packaged project licence files match the repository.
- The completed executable passed isolated dark/light rendering and editor popup open/close checks. No application compilation was run by the assistant.
- Real two-version upgrades, restart/recovery, data preservation and clean-machine acceptance remain pending. These results do not claim those tests passed.

Updates use the existing Ed25519 verification key and ask before restarting. Windows Authenticode signing remains separate and unconfigured.
