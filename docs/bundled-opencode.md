# AI Assistant migration

The assistant now uses bundled LangGraph. See [AI Assistant setup, saved tools,
and package verification](bundled-assistant.md).

Existing OpenCode chat data is retained on disk but is not imported into the new
engine. OpenCode-only grants cannot be reused. Connect a supported provider and
start a new chat after upgrading.
