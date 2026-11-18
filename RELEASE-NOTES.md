# AetherDesk 0.1.3 - build 11

Build 11 reduces background terminal-scanning CPU load, handles occupied desktops clearly, and stages host-agent updates safely.

## Changes in build 11

- Discover desktop terminals from PTY lifecycle notifications instead of scanning all server processes every two seconds. Revalidate one authenticated process per terminal, park the policy worker when neither desktop nor application viewers are attached, and retain a bounded fallback when notifications are unavailable. Reducing shared server CPU load benefits SSH, SSH+X11, Remote Desktop and Remote Applications without changing keyboard or mouse routing.
- Return an occupied-desktop response when another viewer is connected, including retained older brokers. Keep the current viewer responsive and group new transport channels by connection identity so rejected or abandoned attempts cannot contaminate a reconnect.
- Stage agent binaries and libraries side by side under a content identity. Preserve running sessions and existing runtimes. Add a read-only inventory/staging tool and an explicit Open independent host session action; see [safe host-agent upgrades](docs/agent-upgrades.md).
- Keep held-key repeat and mouse capture alive through a delayed client frame using verified input activity. Preserve idle expiry, focus/disconnect release and Ctrl/Shift handling; clicking outside the remote surface revokes capture immediately.

Host-agent fixes take effect in newly started host sessions after rebuilding. Existing retained sessions and their applications keep running with their current agent; disconnect/reconnect preserves that agent too. Client capture improvements require the rebuilt client.

[Earlier release notes](docs/release-history.md).
