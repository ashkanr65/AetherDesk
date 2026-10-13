class RemoteDirectoryUpdate {
  const RemoteDirectoryUpdate({
    required this.paths,
    required this.visibleOutput,
  });

  final List<String> paths;
  final String visibleOutput;
}

/// Incrementally extracts OSC 7 directory reports. Terminal output may split
/// an escape sequence across any number of native-read chunks.
class RemoteDirectoryTracker {
  static const _prefix = '\u001b]7;';

  String _pending = '';

  RemoteDirectoryUpdate process(String output) {
    _pending += output;
    final paths = <String>[];
    final visible = StringBuffer();

    while (_pending.isNotEmpty) {
      final start = _pending.indexOf(_prefix);
      if (start < 0) {
        var retained = 0;
        final maximum = _pending.length < _prefix.length - 1
            ? _pending.length
            : _prefix.length - 1;
        for (var length = maximum; length > 0; length--) {
          if (_prefix.startsWith(
            _pending.substring(_pending.length - length),
          )) {
            retained = length;
            break;
          }
        }
        visible.write(_pending.substring(0, _pending.length - retained));
        _pending = _pending.substring(_pending.length - retained);
        break;
      }

      visible.write(_pending.substring(0, start));
      final bell = _pending.indexOf('\u0007', start + _prefix.length);
      final stringTerminator = _pending.indexOf(
        '\u001b\\',
        start + _prefix.length,
      );
      final end = bell < 0
          ? stringTerminator
          : stringTerminator < 0
          ? bell
          : bell < stringTerminator
          ? bell
          : stringTerminator;
      if (end < 0) {
        _pending = _pending.substring(start);
        break;
      }

      final uri = Uri.tryParse(_pending.substring(start + _prefix.length, end));
      if (uri != null && uri.scheme == 'file' && uri.path.isNotEmpty) {
        try {
          paths.add(Uri.decodeComponent(uri.path));
        } on FormatException {
          // A malformed OSC report is remote control data, not a connection
          // failure. Consume it and continue parsing later valid reports.
        }
      }
      final terminatorLength = end == stringTerminator ? 2 : 1;
      _pending = _pending.substring(end + terminatorLength);
    }

    if (_pending.length > 4096) {
      _pending = '';
    }
    return RemoteDirectoryUpdate(
      paths: paths,
      visibleOutput: visible.toString(),
    );
  }
}

/// Quotes one path as a single POSIX shell word. Newlines and NUL bytes are
/// rejected so pasted text cannot inject a second terminal command.
String quoteRemoteDirectory(String path) {
  if (path.contains('\u0000') || path.contains('\r') || path.contains('\n')) {
    throw const FormatException('The remote directory contains a line break.');
  }
  return "'${path.replaceAll("'", "'\"'\"'")}'";
}

/// Builds the argument for `cd` while retaining the two familiar HOME forms.
/// Every other value remains one literal shell word.
String remoteDirectoryShellArgument(String path) {
  if (path == '~') return r'"$HOME"';
  if (path.startsWith('~/')) {
    return '${r'"$HOME"'}/${quoteRemoteDirectory(path.substring(2))}';
  }
  return quoteRemoteDirectory(path);
}
