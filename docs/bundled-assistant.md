# Bundled AI Assistant and reusable tools

## Connect a model

1. Open **AI Assistant** in the left sidebar. Your current terminal or remote
   desktop stays visible.
2. Select the **key icon** (Manage providers), search the alphabetical list, and select a
   provider. For NVIDIA, enter your NVIDIA API key and keep the default API URL
   `https://integrate.api.nvidia.com/v1`. For another compatible service, choose
   **OpenAI-compatible API**, enter its base URL (including `/v1` if required),
   and its API key if required. Remote APIs require HTTPS; local model servers
   can use HTTP on localhost.
3. Select a **Model** from the searchable alphabetical list. You can enter a
   model ID in the connection dialog when the provider does not list models.
   Choose a chat model supporting function/tool calls for file and tool actions.
4. Under **Workspace**, select a Windows folder or an already connected
   SSH host and its folder, then select **Use workspace**.
5. Type a request. **Enter** sends it; **Ctrl+Enter** inserts a new line.
   Review file changes and commands under **Action details**, then select
   **Allow this message** to approve all actions for that request, **Only this
   action** for one operation, or **Allow all in this chat** for this chat's
   subsequent requests. **Deny** declines the pending action. **Stop** cancels
   the current response. Chat approval is kept in memory, is isolated from other
   chats/workspaces, and resets when the assistant runtime restarts. Select
   **All actions allowed · Ask again** to revoke it. An action already running
   is not undone by revoking approval.

Drag the sidebar's right edge to resize the AI, Logs, Tools or other left panel.
Double-click the edge to restore its default width. The active connection and
chat draft stay open while resizing or switching sidebar panels.

The key icon opens an alphabetical **Connected services** list. Select a service
to choose one of its models without re-entering its key. Its edit icon opens
connection settings; its trash icon removes the saved API. The searchable provider
selector below adds another service. **Remove API** is also available while editing. This stops the backend holding the old key, deletes its Windows
credential and saved endpoint/model configuration, and reloads the remaining
providers. Conversations and reusable tools are kept.

The **history icon** lists saved conversations. Its trash buttons delete
individual chats; **Clear all history** deletes all LangGraph chats and attached
screens after confirmation. Stop an active response before deleting history.
Deleting history does not undo file edits or remove provider keys and saved tools.

## Provider recovery

AetherDesk retries transient HTTP 429 and 500/502/503/504/529 failures before
an answer is delivered, including errors sent inside a stream. It keeps the
selected provider/model, honours Retry-After, and displays retry progress with
Stop available. There are at most three retries and 60 seconds of total retry
waiting. Longer provider wait times and known daily/account quota exhaustion
are reported without repeated requests. Authentication and request-format
errors are not retried. Tool actions are never replayed by this recovery.

Provider streams allow up to 120 seconds of network inactivity for slower
models. Nullable finish/usage frames are accepted. A failure after visible
answer text remains a failed response rather than replacing that text.

## Connection screens and app actions

Choose a model supporting image input, then select **Attach connection screen**.
The captured connection appears as a preview; it is sent with the next message,
not when captured. **Remove screen** discards that attachment. Remote desktop
capture uses the decoded remote frame, including fullscreen/multiple monitors.
SSH/SFTP capture includes the visible AetherDesk connection pane. Separate X11
application windows outside AetherDesk are not included. Screens are reduced to
at most 1600 pixels on the longest edge and a 1 MiB PNG payload.

The assistant can also request an approved capture of an explicitly identified
active connection. Image data goes only to the selected provider along with the
chat; text-only models cannot interpret it. Screen and file text is treated as
untrusted evidence, never as instructions granting permissions.

App tools list and select existing connections, show Tools/Logs, open files in
the popup editor or associated local application, and navigate local/remote
folders. Local folders open in Windows Explorer when no connection file pane is
available. File opens remain inside the selected assistant workspace. Selecting
an AetherDesk tab does not change the assistant's working folder or SSH target.
File edits, local/remote commands and reusable tools retain their existing
revision checks and use the selected approval scope.

The engine is LangGraph. NVIDIA, Groq and OpenAI-compatible chat APIs connect
directly, without routing through OpenCode. OpenRouter lists both free and paid
models, including manually entered model IDs. There is no price-based filtering:
the user chooses the model. Models explicitly reported as lacking tool support
are excluded because this assistant uses tools. Availability, quotas and charges
are set by the provider; LangGraph does not supply free inference or an offline model.
Ollama is optional for people who already run a local model server. It is not
needed to use the bundled assistant with a hosted API.

Provider keys stay in Windows Credential Manager. Preferences and chat data stay
under `%LOCALAPPDATA%\AetherDesk\assistant`; LangGraph conversations use its
`langgraph` subfolder. Existing OpenCode data is retained on disk, but old
conversations and OpenCode-only grants are not imported into the new engine.
Connect a supported provider and start a new chat after upgrading.

The selected provider receives your messages and file contents the agent reads.
Workspace reads are confined to the chosen folder. Writes check the file revision
last read before saving. Approved commands have your normal account access and
can access paths outside that folder. Windows commands use PowerShell; SSH
commands use the connected host's shell. Local commands time out after 30 seconds,
SSH commands after 15 seconds, with bounded output. Interactive/background
programs are not supported by these action tools.

Changing workspaces starts a new conversation. A chat keeps its original target
even if the ordinary file explorer changes folders. Stop an active response
before changing chats or targets. Reconnect a disconnected SSH host to resume its
conversation; it never falls back to running an SSH action on Windows.

## Create and use saved tools

For example, ask: "Create a reusable Windows tool that lists the largest files
in the chosen folder. Add a parameter for the number of results and install it
in Tools." Or select an SSH workspace and ask for a remote disk-usage report.

The agent proposes a tool manifest containing its name, target, script and input
fields. Inspect it before allowing installation. Installing only saves the tool;
running it is a separate action covered by the selected approval scope. Installed tools immediately appear in
**Tools** and remain usable without an AI provider connection.

In **Tools**, use **Run**, choose a folder or connected SSH host, fill in the
inputs, and select **Review script**. The final confirmation shows the exact
script and target. Results appear in the sidebar. **Stop** cancels a running
action. **Inspect**, **Import tool**, and confirmed removal are also available.
Updates and runs check the saved revision, so a replaced tool must be reviewed
again. Parameters become quoted environment values rather than script fragments.

This version supports reusable PowerShell and POSIX shell scripts. It does not
load arbitrary native libraries or custom Flutter UI plugins. Scripts can use
tools already available on the selected computer. A script that requires an
external program still needs that program on its target.

Tool manifests are UTF-8 JSON files in `%LOCALAPPDATA%\AetherDesk\tools`:

```json
{
  "schemaVersion": 1,
  "id": "show-message",
  "title": "Show a message",
  "description": "Print a supplied message on this Windows computer.",
  "version": "1.0.0",
  "target": "windows",
  "script": "Write-Output $env:AETHER_TOOL_message",
  "parameters": [
    {"name": "message", "label": "Message", "required": true}
  ]
}
```

Use `target: "ssh"` and POSIX shell syntax for a remote tool, for example
`printf '%s\n' "$AETHER_TOOL_message"`. Manifests are limited to 64 KiB,
scripts to 32,768 characters, and parameters to 16. Tool IDs are lowercase
names such as `project-summary`; each version uses `major.minor.patch`.

## Installer and portable package

Both distributions include the same self-contained `ai/assistant` folder:
embedded CPython, LangGraph, dependency packages, licenses, `server.py`, and
integrity metadata. End users do not install Python, Node, Bun, OpenCode, or pip.
Extract the entire portable ZIP and keep that folder beside `aetherdesk.exe`.
User data stays in the Windows profile, so the application directory can be
read-only. Moving a portable folder to another computer does not move keys,
saved tools or chat history.

The source pin is `tools/build-kit/assistant-runtime.json`: CPython 3.13.16,
LangGraph 1.2.12, and 38 wheels with exact URLs and SHA-256 hashes. It is separate
from AetherDesk's product version in `release.json`. Dependency licenses ship
inside the runtime. Change the pin and rerun verification when updating versions.

`Bundle-Assistant.ps1` is shared by local step 9 and GitHub Actions. It verifies
downloads/cache contents, rejects unsafe archive entries and builds the runtime
without running pip. The default download cache is
`%LOCALAPPDATA%\AetherDesk\build-cache\assistant`; `-CacheDirectory` supports a
prepopulated cache. Installer and ZIP packaging recursively include the same
verified files. Signing updates integrity metadata after verified signing.

`Verify-Assistant.py` checks the complete file inventory and hashes, isolated
Python import paths, source pin and licenses. It starts the runtime with only
Windows system tools on PATH and a fresh profile, checks authenticated health
and session creation, then runs backend tests against the bundled server and
dependencies. Tests cover streaming/Stop, approvals, history recovery, API
errors, transient HTTP/SSE recovery and cancellation, free/paid model discovery, tool compatibility and private HTTP access. `--files-only` verifies
signed/installed files without starting the process.

`Test-Assistant.ps1 -Package <folder>` runs the Flutter-to-bundled-agent
integration test with a local model fixture. It covers chat, local reads,
approved/denied writes, saved tool install/run, and SSH routing through a fake
native SSH backend. It uses no model credits and does not build the application.
Packaging stops if either verification gate fails. The ordinary Flutter suite
skips this integration test when no package is supplied.

```powershell
$package = 'C:\path\to\AetherDesk-portable'
python -X utf8 tools/build-kit/test_assistant_bundle.py
python -X utf8 tools/build-kit/Verify-Assistant.py --package $package
./tools/build-kit/Test-Assistant.ps1 -Package $package
```

Live provider accounts and a real SSH connection still need a release smoke
check. Automated tests use local fixtures, not external free or paid services.

## Runtime isolation

The app verifies the bundle and launches its Python executable by absolute path
with isolated imports and bytecode writes disabled. It binds to `127.0.0.1` with
a random per-process password, validates Host/Origin, bounds requests, and blocks
redirects carrying credentials. It does not load project plugins, install
packages, inherit ambient provider keys, or enable LangSmith tracing.

LangGraph runs a bounded model/action loop. Only AetherDesk's scoped workspace
and saved-tool/app-control bridge is exposed. Reads can run directly; writes, commands,
installation, tool execution, app actions and screen capture use the selected approval scope. Tool execution approvals
include the installed script, and the bridge checks its exact revision again.

Framework/provider references: [LangGraph](https://docs.langchain.com/oss/python/langgraph/overview),
[NVIDIA API reference](https://docs.api.nvidia.com/nim/reference/llm-apis),
[OpenRouter free models](https://openrouter.ai/docs/guides/routing/model-variants/free).
