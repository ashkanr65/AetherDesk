# Codex inside AetherDesk

Available from AetherDesk 0.1.3 Build 9. Install and sign in to the official Codex app or CLI on this Windows computer first. AetherDesk starts the installed Codex app-server privately; the Codex desktop window does not have to remain open. AetherDesk does not copy account tokens or require another model API key. Codex account access, usage limits and model availability still apply.

## Use the embedded chat

1. Open **Preferences → AI Assistant → Use Codex in AetherDesk**, or choose **Use installed Codex** from the assistant's model picker.
2. Models load automatically when you select Codex or reopen its chat. They appear first in the dropdown under **Codex (installed)**. The temporary `default` entry is replaced by the account's preferred model after discovery. Use **Refresh models** in the aligned service-action row to reload; loading and failure details appear beside the selector.
3. Select the Windows folder or connected host using the workspace icon beside the model. Type and send messages in AetherDesk's existing composer. Replies stream into the same chat; Stop, screenshot attachment, chat history and New chat remain available.

Commands and file operations use the explicitly selected workspace. Changing visible connection tabs does not retarget the chat. Action requests use AetherDesk's existing approval controls. Commands run in separate scoped processes, never by typing into your interactive terminal. File reads/writes retain root and revision checks. Commands have the selected account's permissions, so review the displayed command and target before approving.

The embedded client disables native Codex shell/plugin/external MCP tools for its child process and uses AetherDesk's scoped tools instead. Native permission requests outside that tool boundary are declined. These overrides do not change your normal Codex configuration. Disabling Tools blocks tool execution. Codex manages context and answer limits; the service's maximum response time and supported reasoning effort remain configurable.

Each AetherDesk chat keeps its own Codex conversation identity. Switching provider, workspace or Tools mode starts a fresh Codex context using bounded prior AetherDesk conversation text. A new chat has an independent identity and empty transcript. An unavailable or incompatible saved Codex conversation fails visibly rather than silently rerunning a command in a new session. Conversations may also be recorded by the installed Codex's normal history system.

Stop, closing the chat and application shutdown cancel the owned client process and in-flight scoped action. Cancellation cannot undo a command or file write that already completed; inspect tool results before retrying an interrupted action. Your current Codex desktop chat is not automatically imported or taken over.

## Optional: work from Codex's own interface

The **External Codex connector…** button in AI Assistant settings opens a separate optional MCP bridge. This is not required for the embedded chat.

Enable the connector, keep Read-only enabled initially, and explicitly share a connection or Windows folder. Shared connections expose terminal output, root-folder files and remote desktop screenshots. Shares reset when AetherDesk exits. Click **Connect to installed Codex** to register the packaged `aetherdesk-codex.exe` companion under the name `aetherdesk`. This changes only that named entry in your Codex MCP configuration. Refresh Codex's MCP tools or start a new Codex chat if it retains an old tool catalog.

The external bridge listens only on loopback, authenticates each request using a Windows-user-protected credential and exposes eight bounded tools. Commands and writes remain blocked in Read-only mode; otherwise each requires Allow once inside AetherDesk. Disabling the bridge or revoking a share cancels queued/running access. No SSH passwords, private keys or provider API keys are exposed through its tools.

## Troubleshooting and verification

- **Codex unavailable/not signed in:** install or update Codex and sign in there, then retry model discovery. Login is owned by Codex; do not paste an account token into AetherDesk's API-key fields.
- **No models/usage limit:** check the Codex account's access and limits. Refresh does not buy credits or bypass account restrictions.
- **Unsupported protocol/configuration:** update the installed CLI. Dynamic tools use Codex's experimental app-server API; an incompatible CLI fails visibly. Embedded mode stops if it cannot safely disable an inherited MCP server identifier.
- **External companion missing:** use the complete Build 9 package, with `aetherdesk-codex.exe` beside `AetherDesk.exe`.

Compiler regressions cover framing, input schemas, exact workspace checks, denials, root restrictions, approval UI and revocation. Packaging exercises the real companion without model access. The optional ignored native test `installed_codex_streams_and_resumes_inside_the_assistant` checks streamed conversation/resume and an explicitly approved disposable file read, then archives its own test conversations; it is intentionally excluded from ordinary builds. Real host/Cadence acceptance remains separate.

Official protocol documentation: [Codex App Server](https://learn.chatgpt.com/docs/app-server).
