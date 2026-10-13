# AetherDesk native ABI

The Flutter client uses an existing authenticated and host-verified SSH connection
to start its embedded Linux desktop. Network, agent bootstrap, clipboard, and
decoder processing run on the Rust runtime. The Windows plugin registers a real
Flutter pixel-buffer texture and consumes the newest complete BGRA frame.

## Session functions

- `ad_remote_start(ssh_session_id, config_json)` returns an owned session ID.
- `ad_remote_command(session_id, command_json)` queues input, resize, or clipboard.
- `ad_remote_status(session_id)` returns JSON state and consumes the pending text clipboard update.
- `ad_remote_acquire_frame(session_id)` transfers ownership of the latest frame, or returns null.
- `ad_remote_release_frame(frame)` releases exactly one acquired frame.
- `ad_remote_close(session_id)` terminates the remote desktop and releases its worker; repeated calls are safe.

Release returned strings with `ad_free_string`. Failure returns null or -1 and
sets the calling thread's `ad_last_error`. Frame buffers remain valid after the
session closes, until the native consumer releases them. Flutter retains an
immutable RGBA snapshot until its raster callback releases that snapshot.

```json
{
  "agentPath": "C:/Program Files/AetherDesk/agent/linux-x86_64/ad-agent",
  "runtimeDirectory": "C:/Program Files/AetherDesk/agent/linux-x86_64",
  "agentVersion": "0.1.0",
  "desktopEnvironment": "xfce",
  "width": 1280,
  "height": 720
}
```

Commands use `{"type":"resize","width":1280,"height":720}`,
`{"type":"clipboard","text":"hello"}`, or
`{"type":"input","event":{"Keyboard":{"scan_code":30,"pressed":true,"modifiers":0,"repeat":false}}}`.
Input events follow the root `ad-stream-protocol::InputMessage` schema.

The current Flutter path negotiates H.264 at 30 FPS with audio disabled, bounds
dimensions to 7680x4320, and keeps only the newest decoded presentation snapshot.
It does not yet expose retained-session resume, file drag-and-drop, or audio.
The standalone RDP/VNC functions still report their explicit unavailable errors.
