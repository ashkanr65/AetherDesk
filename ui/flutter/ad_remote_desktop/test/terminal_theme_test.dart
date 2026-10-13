import 'dart:io';

import 'package:ad_remote_desktop/terminal_theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('uses a 100,000 line bounded scrollback by default', () {
    final manager = TerminalThemeManager(
      settingsFile: File('${Directory.systemTemp.path}/unused-terminal.json'),
    );
    expect(manager.appearance.scrollbackLines, 100000);
    expect(manager.appearance.preset, TerminalThemePreset.mobaXterm);
  });

  test('persists user-selectable preset and semantic colors', () async {
    final directory = await Directory.systemTemp.createTemp('aether-theme-');
    final file = File('${directory.path}/terminal-theme.json');
    try {
      final manager = TerminalThemeManager(settingsFile: file);
      manager.applyPreset(TerminalThemePreset.light);
      manager.updateSemanticColor(
        TerminalSemanticRole.error,
        const Color(0xFF123456),
      );
      manager.updateFontSize(16);
      manager.updateScrollbackLines(120000);
      await manager.flush();

      final restored = TerminalThemeManager(settingsFile: file);
      await restored.load();
      expect(restored.appearance.preset, TerminalThemePreset.light);
      expect(
        restored.appearance.palette.semantic(TerminalSemanticRole.error),
        const Color(0xFF123456),
      );
      expect(restored.appearance.fontSize, 16);
      expect(restored.appearance.scrollbackLines, 120000);
    } finally {
      if (await directory.exists()) {
        await directory.delete(recursive: true);
      }
    }
  });

  test('color-blind preset uses non-red/green-only semantic distinctions', () {
    final palette = TerminalPalettes.forPreset(TerminalThemePreset.colorBlind);
    expect(
      palette.semantic(TerminalSemanticRole.error),
      isNot(palette.semantic(TerminalSemanticRole.success)),
    );
    expect(
      palette.semantic(TerminalSemanticRole.authentication),
      isNot(palette.semantic(TerminalSemanticRole.warning)),
    );
  });
}
