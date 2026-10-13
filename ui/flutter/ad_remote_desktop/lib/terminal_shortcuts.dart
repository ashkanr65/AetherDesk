import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:xterm2/xterm.dart';

/// Reserve unshifted Ctrl+A for the remote shell's Readline binding.
Map<ShortcutActivator, Intent> aetherTerminalShortcuts() {
  final shortcuts = Map<ShortcutActivator, Intent>.of(defaultTerminalShortcuts);
  shortcuts.removeWhere(
    (activator, _) =>
        activator is SingleActivator &&
        activator.trigger == LogicalKeyboardKey.keyA &&
        activator.control &&
        !activator.shift &&
        !activator.alt,
  );
  shortcuts.addAll({
    const SingleActivator(LogicalKeyboardKey.keyA, control: true, shift: true):
        const SelectAllTextIntent(SelectionChangedCause.keyboard),
    const SingleActivator(LogicalKeyboardKey.keyV, control: true, shift: true):
        const PasteTextIntent(SelectionChangedCause.keyboard),
    const SingleActivator(LogicalKeyboardKey.insert, shift: true):
        const PasteTextIntent(SelectionChangedCause.keyboard),
    const SingleActivator(LogicalKeyboardKey.insert, control: true):
        CopySelectionTextIntent.copy,
  });
  return shortcuts;
}
