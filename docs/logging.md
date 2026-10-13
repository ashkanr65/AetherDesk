# Application logging

Open **Logs** in the sidebar for Application, Host, Audit and Problems views.
Search, severity/source filters, follow mode and JSONL export are available.
The separate security audit remains authoritative and unchanged.

## Files and ownership

Under `%LOCALAPPDATA%\AetherDesk\logs`:

- `app-<process>-<isolate>.log`: Flutter errors, native-call failures, notices, X11 and assistant diagnostics.
- `core-<process>.log`: Rust tracing events, via a bounded background writer.
- `native-<process>.log`: Windows presentation and monitor events.
- Each file rotates at about 10 MB, retaining five generations (`.1` through `.5`).

Producers use separate files to prevent Windows file-handle and rotation races.
The application view merges the newest 24 files, reading at most 256 KiB per file
and showing at most 2,000 records. Older files remain available in **Open log folder**;
there is currently no age-based pruning across old process IDs.
Dart and Rust writer queues are bounded to 256 entries; overload drops diagnostics
instead of stalling input/video, with a dropped-record notice on subsequent writes.

Records use `ts`, `level`, `source`, `msg` and optional context/stack fields.
Set `AETHERDESK_LOG_LEVEL` to a Rust tracing filter before launch to change core verbosity.
This is event logging, not a per-frame benchmark or a physical-display latency measurement.

## Host logs

The Host view fetches the last 256 KiB of
`~/.aetherdesk/sessions/<desktop-session-id>/agent.log` through the existing SSH connection.
Session identifiers are validated before constructing the fixed command. Concurrent
fetches are suppressed; responses for a previously selected session are ignored.
Follow refreshes every eight seconds. Host records stay in memory unless exported.
Currently only timestamped tracing records are parsed; arbitrary subprocess output
can be inspected in the original host file.

## Privacy and export

Known credential fields, authorization values, private-key blocks and agent endpoint
tokens are redacted. Avoid adding clipboard/chat contents or arbitrary commands to
diagnostics. Redaction is defense in depth, not a guarantee that every possible secret
in third-party error text is recognized; review exports before sharing them.

Use **Export visible records** to save the selected filtered records as JSONL.
For full recent client history, attach the relevant files from **Open log folder**.
