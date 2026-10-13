import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:xterm2/xterm.dart';

enum TerminalSemanticRole {
  error('Error'),
  warning('Warning'),
  success('Success'),
  information('Information'),
  debug('Debug'),
  connection('Connection'),
  disconnection('Disconnection'),
  fileTransfer('File transfer'),
  authentication('Authentication'),
  prompt('Shell prompt'),
  systemMessage('System message');

  const TerminalSemanticRole(this.label);
  final String label;
}

enum TerminalThemePreset {
  mobaXterm('MobaXterm'),
  dark('Aether Dark'),
  light('Aether Light'),
  colorBlind('Color-blind friendly');

  const TerminalThemePreset(this.label);
  final String label;
}

// Memoized per palette: building a fresh xterm theme on every widget rebuild
// invalidates xterm's paragraph cache and forces a full repaint.
final Map<AetherTerminalPalette, TerminalTheme> _xtermThemes =
    Map<AetherTerminalPalette, TerminalTheme>.identity();

@immutable
class AetherTerminalPalette {
  const AetherTerminalPalette({
    required this.background,
    required this.foreground,
    required this.cursor,
    required this.selection,
    required this.black,
    required this.red,
    required this.green,
    required this.yellow,
    required this.blue,
    required this.magenta,
    required this.cyan,
    required this.white,
    required this.brightBlack,
    required this.brightRed,
    required this.brightGreen,
    required this.brightYellow,
    required this.brightBlue,
    required this.brightMagenta,
    required this.brightCyan,
    required this.brightWhite,
    required this.semanticColors,
  });

  final Color background;
  final Color foreground;
  final Color cursor;
  final Color selection;
  final Color black;
  final Color red;
  final Color green;
  final Color yellow;
  final Color blue;
  final Color magenta;
  final Color cyan;
  final Color white;
  final Color brightBlack;
  final Color brightRed;
  final Color brightGreen;
  final Color brightYellow;
  final Color brightBlue;
  final Color brightMagenta;
  final Color brightCyan;
  final Color brightWhite;
  final Map<TerminalSemanticRole, Color> semanticColors;

  Color semantic(TerminalSemanticRole role) => semanticColors[role]!;

  TerminalTheme get xtermTheme =>
      _xtermThemes.putIfAbsent(this, _buildXtermTheme);

  TerminalTheme _buildXtermTheme() => TerminalTheme(
    cursor: cursor,
    selection: selection,
    foreground: foreground,
    background: background,
    black: black,
    red: red,
    green: green,
    yellow: yellow,
    blue: blue,
    magenta: magenta,
    cyan: cyan,
    white: white,
    brightBlack: brightBlack,
    brightRed: brightRed,
    brightGreen: brightGreen,
    brightYellow: brightYellow,
    brightBlue: brightBlue,
    brightMagenta: brightMagenta,
    brightCyan: brightCyan,
    brightWhite: brightWhite,
    searchHitBackground: brightYellow,
    searchHitBackgroundCurrent: brightGreen,
    searchHitForeground: black,
  );

  AetherTerminalPalette copyWithSemantic(
    TerminalSemanticRole role,
    Color color,
  ) => AetherTerminalPalette(
    background: background,
    foreground: foreground,
    cursor: cursor,
    selection: selection,
    black: black,
    red: red,
    green: green,
    yellow: yellow,
    blue: blue,
    magenta: magenta,
    cyan: cyan,
    white: white,
    brightBlack: brightBlack,
    brightRed: brightRed,
    brightGreen: brightGreen,
    brightYellow: brightYellow,
    brightBlue: brightBlue,
    brightMagenta: brightMagenta,
    brightCyan: brightCyan,
    brightWhite: brightWhite,
    semanticColors: {...semanticColors, role: color},
  );
}

@immutable
class TerminalAppearance {
  const TerminalAppearance({
    required this.preset,
    required this.palette,
    this.fontSize = 13,
    this.lineHeight = 1.2,
    this.scrollbackLines = 100000,
    this.autoClassify = true,
  });

  final TerminalThemePreset preset;
  final AetherTerminalPalette palette;
  final double fontSize;
  final double lineHeight;
  final int scrollbackLines;
  final bool autoClassify;

  TerminalAppearance copyWith({
    TerminalThemePreset? preset,
    AetherTerminalPalette? palette,
    double? fontSize,
    double? lineHeight,
    int? scrollbackLines,
    bool? autoClassify,
  }) => TerminalAppearance(
    preset: preset ?? this.preset,
    palette: palette ?? this.palette,
    fontSize: fontSize ?? this.fontSize,
    lineHeight: lineHeight ?? this.lineHeight,
    scrollbackLines: scrollbackLines ?? this.scrollbackLines,
    autoClassify: autoClassify ?? this.autoClassify,
  );
}

class TerminalPalettes {
  const TerminalPalettes._();

  static AetherTerminalPalette forPreset(TerminalThemePreset preset) =>
      switch (preset) {
        TerminalThemePreset.mobaXterm => _mobaXterm,
        TerminalThemePreset.dark => _dark,
        TerminalThemePreset.light => _light,
        TerminalThemePreset.colorBlind => _colorBlind,
      };

  static const _mobaXterm = AetherTerminalPalette(
    background: Color(0xFF111315),
    foreground: Color(0xFFE7E7E7),
    cursor: Color(0xFFF2F2F2),
    selection: Color(0x665B8DEF),
    black: Color(0xFF171717),
    red: Color(0xFFE24848),
    green: Color(0xFF33C266),
    yellow: Color(0xFFD9A72E),
    blue: Color(0xFF4B8FE2),
    magenta: Color(0xFFC678DD),
    cyan: Color(0xFF46B8C7),
    white: Color(0xFFD8D8D8),
    brightBlack: Color(0xFF73777E),
    brightRed: Color(0xFFFF5F5F),
    brightGreen: Color(0xFF55E182),
    brightYellow: Color(0xFFFFC857),
    brightBlue: Color(0xFF76AEED),
    brightMagenta: Color(0xFFD99AE8),
    brightCyan: Color(0xFF72D5DF),
    brightWhite: Color(0xFFFFFFFF),
    semanticColors: {
      TerminalSemanticRole.error: Color(0xFFFF5F5F),
      TerminalSemanticRole.warning: Color(0xFFFFC857),
      TerminalSemanticRole.success: Color(0xFF55E182),
      TerminalSemanticRole.information: Color(0xFF72D5DF),
      TerminalSemanticRole.debug: Color(0xFF8B9098),
      TerminalSemanticRole.connection: Color(0xFF69F0AE),
      TerminalSemanticRole.disconnection: Color(0xFFFF7B7B),
      TerminalSemanticRole.fileTransfer: Color(0xFFC9A7FF),
      TerminalSemanticRole.authentication: Color(0xFF76AEED),
      TerminalSemanticRole.prompt: Color(0xFF8BE9FD),
      TerminalSemanticRole.systemMessage: Color(0xFFF1E67A),
    },
  );

  static const _dark = AetherTerminalPalette(
    background: Color(0xFF0B0D12),
    foreground: Color(0xFFDDE2EA),
    cursor: Color(0xFFE7EAF0),
    selection: Color(0x665E6AD2),
    black: Color(0xFF161A22),
    red: Color(0xFFE0525E),
    green: Color(0xFF45C486),
    yellow: Color(0xFFD8AC4A),
    blue: Color(0xFF5C8FEA),
    magenta: Color(0xFFB778E6),
    cyan: Color(0xFF48B8C8),
    white: Color(0xFFD5DAE3),
    brightBlack: Color(0xFF727986),
    brightRed: Color(0xFFFF6B76),
    brightGreen: Color(0xFF63DCA0),
    brightYellow: Color(0xFFF1C35C),
    brightBlue: Color(0xFF79A7F4),
    brightMagenta: Color(0xFFCA92F1),
    brightCyan: Color(0xFF66D2DF),
    brightWhite: Color(0xFFFFFFFF),
    semanticColors: {
      TerminalSemanticRole.error: Color(0xFFFF6B76),
      TerminalSemanticRole.warning: Color(0xFFF1C35C),
      TerminalSemanticRole.success: Color(0xFF63DCA0),
      TerminalSemanticRole.information: Color(0xFF66D2DF),
      TerminalSemanticRole.debug: Color(0xFF858C98),
      TerminalSemanticRole.connection: Color(0xFF72E6A8),
      TerminalSemanticRole.disconnection: Color(0xFFFF858E),
      TerminalSemanticRole.fileTransfer: Color(0xFFC49AF3),
      TerminalSemanticRole.authentication: Color(0xFF79A7F4),
      TerminalSemanticRole.prompt: Color(0xFF8AB4F8),
      TerminalSemanticRole.systemMessage: Color(0xFFE6D878),
    },
  );

  static const _light = AetherTerminalPalette(
    background: Color(0xFFF8F9FC),
    foreground: Color(0xFF20242B),
    cursor: Color(0xFF24282F),
    selection: Color(0x665669D7),
    black: Color(0xFF20242B),
    red: Color(0xFFB4232F),
    green: Color(0xFF167345),
    yellow: Color(0xFF8A6410),
    blue: Color(0xFF235DAA),
    magenta: Color(0xFF7C3AA8),
    cyan: Color(0xFF0C6E7A),
    white: Color(0xFFE3E6EB),
    brightBlack: Color(0xFF555D68),
    brightRed: Color(0xFFD03442),
    brightGreen: Color(0xFF12804A),
    brightYellow: Color(0xFFA87609),
    brightBlue: Color(0xFF2D6DC1),
    brightMagenta: Color(0xFF9349C3),
    brightCyan: Color(0xFF0D7F8C),
    brightWhite: Color(0xFFFFFFFF),
    semanticColors: {
      TerminalSemanticRole.error: Color(0xFFB4232F),
      TerminalSemanticRole.warning: Color(0xFF8A6410),
      TerminalSemanticRole.success: Color(0xFF167345),
      TerminalSemanticRole.information: Color(0xFF0C6E7A),
      TerminalSemanticRole.debug: Color(0xFF5E6671),
      TerminalSemanticRole.connection: Color(0xFF0B7442),
      TerminalSemanticRole.disconnection: Color(0xFFB4232F),
      TerminalSemanticRole.fileTransfer: Color(0xFF7041A5),
      TerminalSemanticRole.authentication: Color(0xFF235DAA),
      TerminalSemanticRole.prompt: Color(0xFF174EA6),
      TerminalSemanticRole.systemMessage: Color(0xFF705B0A),
    },
  );

  // Okabe-Ito inspired semantic colors keep status distinguishable for common
  // red/green deficiencies. Status icons and labels remain the primary signal.
  static const _colorBlind = AetherTerminalPalette(
    background: Color(0xFF101418),
    foreground: Color(0xFFE4E9EE),
    cursor: Color(0xFFFFFFFF),
    selection: Color(0x6668A8D8),
    black: Color(0xFF151A20),
    red: Color(0xFFD55E00),
    green: Color(0xFF009E73),
    yellow: Color(0xFFE69F00),
    blue: Color(0xFF0072B2),
    magenta: Color(0xFFCC79A7),
    cyan: Color(0xFF56B4E9),
    white: Color(0xFFDDE3E8),
    brightBlack: Color(0xFF7E8790),
    brightRed: Color(0xFFFF7A26),
    brightGreen: Color(0xFF2BC59A),
    brightYellow: Color(0xFFF3B83C),
    brightBlue: Color(0xFF54A3D4),
    brightMagenta: Color(0xFFE093C0),
    brightCyan: Color(0xFF82CCED),
    brightWhite: Color(0xFFFFFFFF),
    semanticColors: {
      TerminalSemanticRole.error: Color(0xFFFF7A26),
      TerminalSemanticRole.warning: Color(0xFFF3B83C),
      TerminalSemanticRole.success: Color(0xFF2BC59A),
      TerminalSemanticRole.information: Color(0xFF82CCED),
      TerminalSemanticRole.debug: Color(0xFF8A949D),
      TerminalSemanticRole.connection: Color(0xFF2BC59A),
      TerminalSemanticRole.disconnection: Color(0xFFFF7A26),
      TerminalSemanticRole.fileTransfer: Color(0xFFE093C0),
      TerminalSemanticRole.authentication: Color(0xFF54A3D4),
      TerminalSemanticRole.prompt: Color(0xFF82CCED),
      TerminalSemanticRole.systemMessage: Color(0xFFF3B83C),
    },
  );
}

class TerminalThemeManager extends ChangeNotifier {
  TerminalThemeManager({File? settingsFile})
    : _settingsFile = settingsFile ?? _systemSettingsFile(),
      _appearance = TerminalAppearance(
        preset: TerminalThemePreset.mobaXterm,
        palette: TerminalPalettes.forPreset(TerminalThemePreset.mobaXterm),
      );

  final File _settingsFile;
  TerminalAppearance _appearance;
  Future<void> _pendingWrite = Future<void>.value();

  TerminalAppearance get appearance => _appearance;

  static File _systemSettingsFile() {
    final localAppData = Platform.environment['LOCALAPPDATA'];
    final base = localAppData == null || localAppData.isEmpty
        ? Directory.systemTemp
        : Directory(localAppData);
    return File(
      '${base.path}${Platform.pathSeparator}AetherDesk'
      '${Platform.pathSeparator}terminal-theme.json',
    );
  }

  Future<void> load() async {
    if (!await _settingsFile.exists()) return;
    try {
      final decoded = jsonDecode(await _settingsFile.readAsString());
      if (decoded is! Map<String, Object?>) return;
      final presetName = decoded['preset'] as String?;
      final preset = TerminalThemePreset.values.firstWhere(
        (value) => value.name == presetName,
        orElse: () => TerminalThemePreset.mobaXterm,
      );
      var palette = TerminalPalettes.forPreset(preset);
      final semantic = decoded['semanticColors'];
      if (semantic is Map<String, Object?>) {
        for (final role in TerminalSemanticRole.values) {
          final raw = semantic[role.name];
          if (raw is int) palette = palette.copyWithSemantic(role, Color(raw));
        }
      }
      _appearance = TerminalAppearance(
        preset: preset,
        palette: palette,
        fontSize: _boundedDouble(decoded['fontSize'], 10, 28, 13),
        lineHeight: _boundedDouble(decoded['lineHeight'], 1, 2, 1.2),
        scrollbackLines: _boundedInt(
          decoded['scrollbackLines'],
          20000,
          250000,
          100000,
        ),
        autoClassify: decoded['autoClassify'] as bool? ?? true,
      );
      notifyListeners();
    } on Object {
      // A malformed user preference must never prevent the terminal opening.
    }
  }

  void applyPreset(TerminalThemePreset preset) {
    _appearance = _appearance.copyWith(
      preset: preset,
      palette: TerminalPalettes.forPreset(preset),
    );
    _changed();
  }

  void updateSemanticColor(TerminalSemanticRole role, Color color) {
    _appearance = _appearance.copyWith(
      palette: _appearance.palette.copyWithSemantic(role, color),
    );
    _changed();
  }

  void updateFontSize(double value) {
    _appearance = _appearance.copyWith(fontSize: value.clamp(10, 28));
    _changed();
  }

  void updateLineHeight(double value) {
    _appearance = _appearance.copyWith(lineHeight: value.clamp(1, 2));
    _changed();
  }

  void updateScrollbackLines(int value) {
    _appearance = _appearance.copyWith(
      scrollbackLines: value.clamp(20000, 250000),
    );
    _changed();
  }

  void updateAutoClassify(bool value) {
    _appearance = _appearance.copyWith(autoClassify: value);
    _changed();
  }

  void _changed() {
    notifyListeners();
    _pendingWrite = _pendingWrite.then((_) => _save()).catchError((_) {});
  }

  Future<void> flush() => _pendingWrite;

  Future<void> _save() async {
    await _settingsFile.parent.create(recursive: true);
    final semantic = <String, int>{
      for (final role in TerminalSemanticRole.values)
        role.name: _appearance.palette.semantic(role).toARGB32(),
    };
    await _settingsFile.writeAsString(
      const JsonEncoder.withIndent('  ').convert({
        'version': 1,
        'preset': _appearance.preset.name,
        'fontSize': _appearance.fontSize,
        'lineHeight': _appearance.lineHeight,
        'scrollbackLines': _appearance.scrollbackLines,
        'autoClassify': _appearance.autoClassify,
        'semanticColors': semantic,
      }),
      flush: true,
    );
  }

  static double _boundedDouble(
    Object? raw,
    double minimum,
    double maximum,
    double fallback,
  ) {
    if (raw is! num) return fallback;
    return raw.toDouble().clamp(minimum, maximum);
  }

  static int _boundedInt(Object? raw, int minimum, int maximum, int fallback) {
    if (raw is! num) return fallback;
    return raw.toInt().clamp(minimum, maximum);
  }
}
