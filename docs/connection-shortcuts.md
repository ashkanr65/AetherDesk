# Connection keyboard shortcuts

Click the terminal or remote desktop to give it keyboard focus.

SSH and SSH+X11 terminals send Linux shell shortcuts to the remote PTY:
Ctrl+A/E (line start/end), Ctrl+F/B (character movement), Alt+F/B (word
movement), Ctrl+U/K/W (cut to line start/end/previous word), Ctrl+Y (yank),
Ctrl+T (transpose characters), Alt+T (transpose words), and Ctrl+_ (undo;
usually Ctrl+Shift+-). Ctrl+C interrupts, Ctrl+Z suspends, Ctrl+D sends EOF,
and Ctrl+R searches history. The host shell decides these actions; they follow
its active Readline/keymap settings and may differ in full-screen applications.

Ctrl+Shift+C copies terminal selection, Ctrl+V or Ctrl+Shift+V pastes the
Windows clipboard, and Ctrl+Shift+A selects terminal output. Ctrl+Insert
copies; Shift+Insert pastes. Unshifted Ctrl+A is reserved for the remote shell.

Remote desktop forwards physical Ctrl/Alt/Shift chords to the host application,
including a terminal opened inside the remote desktop. The host's keyboard
layout and application define their meaning. Ctrl+Alt+Escape releases keyboard
capture in tab mode. Windows security shortcuts intercepted by the local OS
cannot be forwarded by ordinary application key events.
