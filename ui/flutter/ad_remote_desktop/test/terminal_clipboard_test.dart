import 'package:ad_remote_desktop/terminal_clipboard.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xterm2/xterm.dart';

void main() {
  test('right click pastes clipboard text when nothing is selected', () async {
    var output = '';
    final interaction = TerminalClipboardInteraction(
      terminal: Terminal(onOutput: (value) => output += value),
      controller: TerminalController(),
      readClipboard: () async => 'pasted command',
      writeClipboard: (_) async {},
    );

    final result = await interaction.handleSecondaryClick();

    expect(result, TerminalSecondaryClickResult.pasted);
    expect(output, 'pasted command');
  });

  test('right click ignores an empty clipboard', () async {
    var output = '';
    final interaction = TerminalClipboardInteraction(
      terminal: Terminal(onOutput: (value) => output += value),
      controller: TerminalController(),
      readClipboard: () async => '',
      writeClipboard: (_) async {},
    );

    final result = await interaction.handleSecondaryClick();

    expect(result, TerminalSecondaryClickResult.ignored);
    expect(output, isEmpty);
  });

  test('right click copies and clears a selected terminal range', () async {
    final terminal = Terminal();
    terminal.write('selected text');
    final controller = TerminalController();
    controller.setSelection(
      terminal.buffer.createAnchor(0, 0),
      terminal.buffer.createAnchor(8, 0),
    );
    String? clipboard;
    final interaction = TerminalClipboardInteraction(
      terminal: terminal,
      controller: controller,
      readClipboard: () async => null,
      writeClipboard: (text) async => clipboard = text,
    );

    final result = await interaction.handleSecondaryClick();

    expect(result, TerminalSecondaryClickResult.copied);
    expect(clipboard, 'selected');
    expect(controller.selection, isNull);
  });
}
