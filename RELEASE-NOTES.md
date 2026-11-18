# AetherDesk 0.1.3 - build 12

- Keep Remote Desktop connected when a clipboard transfer contains an unsupported format, invalid data, or an outdated revision. Discard the failed transfer and continue accepting clipboard updates.

- Reduce the Windows package size by shipping GStreamer runtime plugins without its static build libraries.

The host-side fix applies to new sessions using the updated agent. Existing sessions retain their running agent and applications.

[Earlier release notes](docs/release-history.md).
