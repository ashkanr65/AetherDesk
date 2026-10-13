import 'package:xterm2/xterm.dart';

typedef ClipboardTextReader = Future<String?> Function();
typedef ClipboardTextWriter = Future<void> Function(String text);

enum TerminalSecondaryClickResult { copied, pasted, ignored }

/// MobaXterm-style terminal interaction: right-click copies an active
/// selection; otherwise it pastes text from the system clipboard.
class TerminalClipboardInteraction {
  const TerminalClipboardInteraction({
    required this.terminal,
    required this.controller,
    required this.readClipboard,
    required this.writeClipboard,
  });

  final Terminal terminal;
  final TerminalController controller;
  final ClipboardTextReader readClipboard;
  final ClipboardTextWriter writeClipboard;

  Future<TerminalSecondaryClickResult> handleSecondaryClick() async {
    final selection = controller.selectionFor(terminal.buffer);
    if (selection != null) {
      final text = terminal.buffer.getText(selection, true);
      if (text.isNotEmpty) {
        await writeClipboard(text);
      }
      controller.clearSelection();
      return text.isEmpty
          ? TerminalSecondaryClickResult.ignored
          : TerminalSecondaryClickResult.copied;
    }

    final text = await readClipboard();
    if (text == null || text.isEmpty) {
      return TerminalSecondaryClickResult.ignored;
    }
    terminal.paste(text);
    return TerminalSecondaryClickResult.pasted;
  }
}
