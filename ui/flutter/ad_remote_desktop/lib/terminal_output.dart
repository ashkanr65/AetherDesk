import 'terminal_theme.dart';

class TerminalLogClassifier {
  const TerminalLogClassifier();

  static final RegExp _error = RegExp(
    r'(?:^|\s)\*\*(?:error|fatal)\b|\b(error|failed|failure|fatal|panic|denied|refused|rejected|invalid|unreachable|timed?\s*out)\b',
    caseSensitive: false,
  );
  static final RegExp _warning = RegExp(
    r'(?:^|\s)\*\*warn(?:ing)?\b|\b(warn(?:ing)?|deprecated|retrying|degraded|caution)\b',
    caseSensitive: false,
  );
  static final RegExp _disconnect = RegExp(
    r'\b(disconnected|disconnecting|connection\s+closed|connection\s+lost|signed\s+out)\b',
    caseSensitive: false,
  );
  static final RegExp _connection = RegExp(
    r'\b(connected|connecting|connection\s+established|session\s+opened|reconnected)\b',
    caseSensitive: false,
  );
  static final RegExp _authentication = RegExp(
    r'\b(authenticat(?:e|ed|ing|ion)|authorized|public\s+key|host\s+key|fingerprint|passphrase|credential|mfa|fido2)\b',
    caseSensitive: false,
  );
  static final RegExp _fileTransfer = RegExp(
    r'\b(sftp|upload(?:ed|ing)?|download(?:ed|ing)?|transferr(?:ed|ing)|copied|bytes?\s+(?:sent|received))\b',
    caseSensitive: false,
  );
  static final RegExp _success = RegExp(
    r'\b(success(?:ful|fully)?|completed|finished|passed|done|ok)\b',
    caseSensitive: false,
  );
  static final RegExp _information = RegExp(
    r'(?:^|\s)\*\*info\b|\b(info(?:rmation)?|notice|listening|ready|started)\b',
    caseSensitive: false,
  );
  static final RegExp _debug = RegExp(
    r'\b(debug|trace|verbose)\b',
    caseSensitive: false,
  );
  static final RegExp _system = RegExp(
    r'\b(systemd|kernel|service|daemon|system\s+message|last\s+login)\b',
    caseSensitive: false,
  );
  static final RegExp _innovusSection = RegExp(
    r'^\s*(?:#@\s*(?:begin|end)|@{1,2}file\s+\d+:\s*#\s*(?:=+|\d+\.)|(?:reading|loading|running|starting|creating|checking|analyzing|placing|routing|optimizing|exporting)\b)',
    caseSensitive: false,
  );
  static final RegExp _innovusEcho = RegExp(
    r'^\s*@{1,2}file\s+\d+:',
    caseSensitive: false,
  );

  TerminalSemanticRole? classify(String line) {
    if (line.trim().isEmpty) return null;
    if (_error.hasMatch(line)) return TerminalSemanticRole.error;
    if (_warning.hasMatch(line)) return TerminalSemanticRole.warning;
    if (_disconnect.hasMatch(line)) return TerminalSemanticRole.disconnection;
    if (_connection.hasMatch(line)) return TerminalSemanticRole.connection;
    if (_authentication.hasMatch(line)) {
      return TerminalSemanticRole.authentication;
    }
    if (_fileTransfer.hasMatch(line)) return TerminalSemanticRole.fileTransfer;
    if (_success.hasMatch(line)) return TerminalSemanticRole.success;
    if (_information.hasMatch(line)) return TerminalSemanticRole.information;
    if (_innovusSection.hasMatch(line)) {
      return TerminalSemanticRole.systemMessage;
    }
    if (_innovusEcho.hasMatch(line)) return TerminalSemanticRole.debug;
    if (_debug.hasMatch(line)) return TerminalSemanticRole.debug;
    if (_system.hasMatch(line)) return TerminalSemanticRole.systemMessage;
    return null;
  }
}

/// Adds semantic true-color styling to plain line-oriented output.
///
/// Escape sequences supplied by the server always take precedence. Alternate
/// screen applications and active remote SGR styles pass through unchanged,
/// which prevents classification from altering interactive terminal programs.
class TerminalOutputParser {
  TerminalOutputParser({TerminalLogClassifier? classifier})
    : _classifier = classifier ?? const TerminalLogClassifier();

  final TerminalLogClassifier _classifier;
  bool _alternateScreen = false;
  bool _remoteSgrActive = false;

  static final RegExp _controlSequence = RegExp(r'\x1b\[[0-?]*[ -/]*[@-~]');
  static final RegExp _prompt = RegExp(
    r'(\[[^\]\r\n]+@[^\]\r\n]+\s+[^\]\r\n]+\][#$]\s+|@innovus\s+\d+>\s+|[A-Za-z0-9._-]+@[A-Za-z0-9._-]+:[^\r\n]*[#$]\s+)$',
  );

  String transform(
    String chunk,
    AetherTerminalPalette palette, {
    bool enabled = true,
  }) {
    if (chunk.isEmpty || !enabled) return chunk;
    if (chunk.contains('\x1b')) {
      _trackRemoteState(chunk);
      return chunk;
    }
    if (_alternateScreen || _remoteSgrActive) {
      return chunk;
    }
    if (!chunk.contains('\n')) {
      return _styleTrailingPrompt(chunk, palette);
    }

    final output = StringBuffer();
    var start = 0;
    while (start < chunk.length) {
      final newline = chunk.indexOf('\n', start);
      if (newline < 0) {
        output.write(_styleTrailingPrompt(chunk.substring(start), palette));
        break;
      }
      final rawLine = chunk.substring(start, newline);
      final content = rawLine.endsWith('\r')
          ? rawLine.substring(0, rawLine.length - 1)
          : rawLine;
      final carriageReturn = rawLine.endsWith('\r') ? '\r' : '';
      final role = _classifier.classify(content);
      if (role == null || _containsNonTextControl(content)) {
        output.write(content);
      } else {
        output.write(style(content, role, palette));
      }
      output
        ..write(carriageReturn)
        ..write('\n');
      start = newline + 1;
    }
    return output.toString();
  }

  String style(
    String text,
    TerminalSemanticRole role,
    AetherTerminalPalette palette,
  ) {
    final color = palette.semantic(role);
    final value = color.toARGB32();
    final red = (value >> 16) & 0xff;
    final green = (value >> 8) & 0xff;
    final blue = value & 0xff;
    final bold =
        role == TerminalSemanticRole.error ||
        role == TerminalSemanticRole.warning ||
        role == TerminalSemanticRole.prompt;
    return '\x1b[${bold ? '1;' : ''}38;2;$red;$green;${blue}m$text\x1b[0m';
  }

  String _styleTrailingPrompt(String text, AetherTerminalPalette palette) {
    final match = _prompt.firstMatch(text);
    if (match == null || _containsNonTextControl(text)) return text;
    final prompt = match.group(1)!;
    return '${text.substring(0, match.start)}'
        '${style(prompt, TerminalSemanticRole.prompt, palette)}';
  }

  void _trackRemoteState(String chunk) {
    for (final match in _controlSequence.allMatches(chunk)) {
      final sequence = match.group(0)!;
      if (sequence == '\x1b[?1049h' ||
          sequence == '\x1b[?1047h' ||
          sequence == '\x1b[?47h') {
        _alternateScreen = true;
      } else if (sequence == '\x1b[?1049l' ||
          sequence == '\x1b[?1047l' ||
          sequence == '\x1b[?47l') {
        _alternateScreen = false;
      }
      if (sequence.endsWith('m')) {
        final parameters = sequence.substring(2, sequence.length - 1);
        if (parameters.isEmpty ||
            parameters.split(';').any((value) => value == '0')) {
          _remoteSgrActive = false;
        } else {
          _remoteSgrActive = true;
        }
      }
    }
  }

  static bool _containsNonTextControl(String value) {
    for (final unit in value.codeUnits) {
      if (unit < 0x20 && unit != 0x09) return true;
    }
    return false;
  }
}
