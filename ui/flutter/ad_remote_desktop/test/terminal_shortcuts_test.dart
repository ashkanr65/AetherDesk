import 'package:ad_remote_desktop/terminal_shortcuts.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xterm2/xterm.dart';

void main() {
  testWidgets('Linux editing shortcuts reach the SSH PTY unchanged', (
    tester,
  ) async {
    final output = <String>[];
    final terminal = Terminal(onOutput: output.add);
    final controller = TerminalController();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: TerminalView(
            terminal,
            controller: controller,
            autofocus: true,
            shortcuts: aetherTerminalShortcuts(),
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.tap(find.byType(TerminalView));
    await tester.pump();
    for (final entry in {
      LogicalKeyboardKey.keyA: '\x01',
      LogicalKeyboardKey.keyE: '\x05',
      LogicalKeyboardKey.keyF: '\x06',
      LogicalKeyboardKey.keyB: '\x02',
      LogicalKeyboardKey.keyU: '\x15',
      LogicalKeyboardKey.keyK: '\x0b',
      LogicalKeyboardKey.keyW: '\x17',
      LogicalKeyboardKey.keyY: '\x19',
      LogicalKeyboardKey.keyT: '\x14',
      LogicalKeyboardKey.keyC: '\x03',
      LogicalKeyboardKey.keyD: '\x04',
      LogicalKeyboardKey.keyR: '\x12',
      LogicalKeyboardKey.keyZ: '\x1a',
      LogicalKeyboardKey.minus: '\x1f',
    }.entries) {
      output.clear();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(entry.key);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      expect(output.join(), entry.value, reason: entry.key.debugName);
    }
    for (final entry in {
      LogicalKeyboardKey.keyF: '\x1bf',
      LogicalKeyboardKey.keyB: '\x1bb',
      LogicalKeyboardKey.keyT: '\x1bt',
    }.entries) {
      output.clear();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
      await tester.sendKeyEvent(entry.key);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
      expect(output.join(), entry.value, reason: entry.key.debugName);
    }
    output.clear();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.minus, character: '_');
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    expect(output.join(), '\x1f');
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
  });
}
