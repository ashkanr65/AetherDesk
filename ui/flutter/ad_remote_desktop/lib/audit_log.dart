import 'dart:async';
import 'dart:convert';
import 'dart:io';

class AuditEntry {
  const AuditEntry({
    required this.timestamp,
    required this.event,
    required this.result,
    this.profileId,
    this.profileName,
    this.host,
    this.detail,
  });

  final DateTime timestamp;
  final String event;
  final String result;
  final String? profileId;
  final String? profileName;
  final String? host;
  final String? detail;

  Map<String, Object?> toJson() => {
        'timestamp': timestamp.toUtc().toIso8601String(),
        'event': event,
        'result': result,
        if (profileId != null) 'profileId': profileId,
        if (profileName != null) 'profileName': profileName,
        if (host != null) 'host': host,
        if (detail != null) 'detail': detail,
      };

  factory AuditEntry.fromJson(Map<String, Object?> json) => AuditEntry(
        timestamp: DateTime.tryParse(json['timestamp'] as String? ?? '') ??
            DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
        event: json['event'] as String? ?? 'unknown',
        result: json['result'] as String? ?? 'unknown',
        profileId: json['profileId'] as String?,
        profileName: json['profileName'] as String?,
        host: json['host'] as String?,
        detail: json['detail'] as String?,
      );
}

class AuditLog {
  AuditLog._(this._file);

  final File _file;
  Future<void> _pendingWrite = Future<void>.value();
  static const int _maxBytes = 5 * 1024 * 1024;

  factory AuditLog.systemDefault() {
    final localAppData = Platform.environment['LOCALAPPDATA'];
    final root = localAppData == null || localAppData.isEmpty
        ? Directory.current
        : Directory(localAppData);
    return AuditLog._(
      File(
        '${root.path}${Platform.pathSeparator}AetherDesk'
        '${Platform.pathSeparator}audit.jsonl',
      ),
    );
  }

  String get path => _file.path;

  Future<void> record({
    required String event,
    required String result,
    String? profileId,
    String? profileName,
    String? host,
    String? detail,
  }) {
    final operation = _pendingWrite.then(
      (_) => _write(
        event: event,
        result: result,
        profileId: profileId,
        profileName: profileName,
        host: host,
        detail: detail,
      ),
    );
    _pendingWrite = operation.catchError((Object _) {});
    return operation;
  }

  Future<void> _write({
    required String event,
    required String result,
    String? profileId,
    String? profileName,
    String? host,
    String? detail,
  }) async {
    await _file.parent.create(recursive: true);
    await _rotateIfNeeded();
    final entry = AuditEntry(
      timestamp: DateTime.now(),
      event: event,
      result: result,
      profileId: profileId,
      profileName: profileName,
      host: host,
      detail: _sanitized(detail),
    );
    await _file.writeAsString(
      '${jsonEncode(entry.toJson())}\n',
      mode: FileMode.append,
      flush: true,
    );
  }

  Future<List<AuditEntry>> recent({int limit = 300}) async {
    await _pendingWrite;
    if (!await _file.exists()) return const [];
    final lines = await _file.readAsLines();
    final entries = <AuditEntry>[];
    for (final line in lines.reversed.take(limit)) {
      try {
        entries.add(
          AuditEntry.fromJson(
            Map<String, Object?>.from(jsonDecode(line) as Map),
          ),
        );
      } on Object {
        // A single damaged audit line does not hide the remaining history.
      }
    }
    return entries;
  }

  Future<void> _rotateIfNeeded() async {
    if (!await _file.exists() || await _file.length() < _maxBytes) return;
    final previous = File('${_file.path}.1');
    if (await previous.exists()) await previous.delete();
    await _file.rename(previous.path);
  }

  String? _sanitized(String? value) => sanitizeAuditDetail(value);
}

String? sanitizeAuditDetail(String? value) {
  if (value == null) return null;
  // Dart's RegExp does not accept a leading inline `(?i)` flag. Supplying the
  // option explicitly also keeps the capture group valid for replacement.
  final secretAssignment = RegExp(
    r'(password|passphrase|secret)=\S+',
    caseSensitive: false,
  );
  final sanitized = value
      .replaceAllMapped(
        secretAssignment,
        (match) => '${match.group(1)}=<redacted>',
      )
      .replaceAll(RegExp(r'[\r\n]+'), ' ');
  return sanitized.substring(
    0,
    sanitized.length > 600 ? 600 : sanitized.length,
  );
}
