import 'dart:convert';
import 'dart:typed_data';

const maxEditableBytes = 8 * 1024 * 1024;
String fileName(String path) => path.replaceAll(r'\', '/').split('/').last;
String fileExtension(String path) => fileName(path).contains('.')
    ? fileName(path).split('.').last.toLowerCase()
    : '';
String joinRemotePath(String parent, String name) =>
    '${parent.endsWith('/') ? parent : '$parent/'}$name';
String quoteTerminalPath(String path) {
  if (path.contains(RegExp(r'[\x00-\x1f\x7f]')))
    throw const FormatException(
      'This path contains control characters and cannot be pasted into the terminal.',
    );
  return "'${path.replaceAll("'", "'\\''")}'";
}

bool validFileName(String name) =>
    name.isNotEmpty &&
    name != '.' &&
    name != '..' &&
    !name.contains(RegExp(r'[/\\\x00]'));
String safeLocalName(String name) {
  var safe = name
      .replaceAll(RegExp(r'[<>:"/\\|?*\x00-\x1f]'), '_')
      .replaceAll(RegExp(r'[. ]+$'), '');
  if (safe.isEmpty) safe = 'remote-file';
  if (RegExp(
    r'^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)',
    caseSensitive: false,
  ).hasMatch(safe))
    safe = '_$safe';
  return safe.length > 180 ? safe.substring(safe.length - 180) : safe;
}

class TextFileContent {
  TextFileContent(this.text, this.encoding, this.bom);
  final String text, encoding;
  final bool bom;
  static TextFileContent? decode(Uint8List bytes) {
    if (bytes.length > maxEditableBytes) return null;
    if (bytes.length >= 2 &&
        ((bytes[0] == 255 && bytes[1] == 254) ||
            (bytes[0] == 254 && bytes[1] == 255))) {
      if (bytes.length.isOdd) return null;
      final little = bytes[0] == 255;
      final units = <int>[];
      for (var i = 2; i < bytes.length; i += 2) {
        units.add(
          little ? bytes[i] | bytes[i + 1] << 8 : bytes[i] << 8 | bytes[i + 1],
        );
      }
      final text = String.fromCharCodes(units);
      if (!_validText(text)) return null;
      return TextFileContent(text, little ? 'utf16le' : 'utf16be', true);
    }
    if (bytes.contains(0)) return null;
    final bom =
        bytes.length >= 3 &&
        bytes[0] == 239 &&
        bytes[1] == 187 &&
        bytes[2] == 191;
    try {
      final text = utf8.decode(bom ? bytes.sublist(3) : bytes);
      if (!_validText(text) || text.startsWith('%PDF-')) return null;
      return TextFileContent(text, 'utf8', bom);
    } on FormatException {
      return null;
    }
  }

  static bool _validText(String text) {
    final units = text.codeUnits;
    for (var i = 0; i < units.length; i++) {
      final c = units[i];
      if (c < 32 && c != 9 && c != 10 && c != 12 && c != 13 ||
          c >= 127 && c <= 159)
        return false;
      if (c >= 0xd800 && c <= 0xdbff) {
        if (++i >= units.length || units[i] < 0xdc00 || units[i] > 0xdfff)
          return false;
      } else if (c >= 0xdc00 && c <= 0xdfff)
        return false;
    }
    return true;
  }

  Uint8List encode(String content) {
    if (!_validText(content))
      throw const FormatException(
        'The text contains invalid Unicode or binary control characters',
      );
    if (encoding == 'utf8')
      return Uint8List.fromList([
        if (bom) ...[239, 187, 191],
        ...utf8.encode(content),
      ]);
    final little = encoding == 'utf16le';
    return Uint8List.fromList([
      if (bom) ...(little ? [255, 254] : [254, 255]),
      for (final c in content.codeUnits)
        ...(little ? [c & 255, c >> 8] : [c >> 8, c & 255]),
    ]);
  }
}

String languageFor(String path) =>
    const {
      'py': 'python',
      'c': 'c',
      'h': 'cpp',
      'cc': 'cpp',
      'cpp': 'cpp',
      'hpp': 'cpp',
      'rs': 'rust',
      'v': 'verilog',
      'sv': 'systemverilog',
      'js': 'javascript',
      'ts': 'typescript',
      'json': 'json',
      'html': 'html',
      'css': 'css',
      'md': 'markdown',
      'yaml': 'yaml',
      'yml': 'yaml',
      'sh': 'shell',
      'bash': 'shell',
      'ps1': 'powershell',
      'sql': 'sql',
      'xml': 'xml',
      'toml': 'ini',
      'ini': 'ini',
      'tcl': 'tcl',
      'java': 'java',
      'go': 'go',
    }[fileExtension(path)] ??
    'plaintext';
