import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

/// Severity of an application log record. Ordered so `level >= warn` can
/// select the Problems view.
enum AppLogLevel {
  debug(0, 'debug'),
  info(1, 'info'),
  warn(2, 'warn'),
  error(3, 'error');

  const AppLogLevel(this.rank, this.label);
  final int rank;
  final String label;

  static AppLogLevel parse(String? value) => switch (value?.toLowerCase()) {
    'debug' => AppLogLevel.debug,
    'warn' || 'warning' => AppLogLevel.warn,
    'error' || 'fatal' || 'critical' => AppLogLevel.error,
    _ => AppLogLevel.info,
  };
}

/// Known record producers. Host records are fetched on demand (the host
/// writes its own `agent.log`).
abstract final class AppLogSource {
  static const String coreRust = 'core.rust';
  static const String coreFfi = 'core.ffi';
  static const String uiDart = 'ui.dart';
  static const String uiNative = 'ui.native';
  static const String editor = 'editor';
  static const String assistant = 'assistant';
  static const String x11 = 'x11';
  static const String hostAgent = 'host.agent';
}

class AppLogRecord {
  const AppLogRecord({
    required this.timestamp,
    required this.level,
    required this.source,
    required this.message,
    this.target,
    this.session,
    this.profile,
    this.host,
    this.error,
    this.seq,
  });

  final DateTime timestamp;
  final AppLogLevel level;
  final String source;
  final String message;
  final String? target;
  final String? session;
  final String? profile;
  final String? host;
  final String? error;
  final int? seq;

  Map<String, Object?> toJson() => {
    'ts': timestamp.toUtc().toIso8601String(),
    'level': level.label,
    'source': source,
    if (target != null) 'target': target,
    if (session != null) 'session': session,
    if (profile != null) 'profile': profile,
    if (host != null) 'host': host,
    'msg': message,
    if (error != null) 'error': error,
    if (seq != null) 'seq': seq,
  };

  /// Parses one line of `app.log`. Accepts the Dart schema, the
  /// `tracing-subscriber` JSON schema written by the Rust core, and the
  /// host's `agent.log` fmt lines, so the Logs panel can merge all sides.
  static AppLogRecord? tryParse(String line, {String? defaultSource}) {
    final trimmed = line.trim();
    if (trimmed.isEmpty) return null;
    if (trimmed.startsWith('{')) {
      try {
        final json = Map<String, Object?>.from(jsonDecode(trimmed) as Map);
        final fields = json['fields'] is Map
            ? Map<String, Object?>.from(json['fields'] as Map)
            : const <String, Object?>{};
        final levelValue = json['level'];
        final message =
            (json['msg'] ?? fields['message'] ?? json['message'] ?? '')
                .toString();
        if (message.isEmpty) return null;
        final target = (json['target'] ?? fields['target'])?.toString();
        return AppLogRecord(
          timestamp:
              _parseTimestamp(json['ts'] ?? json['timestamp']) ??
              DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
          level: AppLogLevel.parse(levelValue?.toString()),
          source: (json['source'] ?? defaultSource ?? AppLogSource.coreRust)
              .toString(),
          message: sanitizeLogText(message),
          target: target,
          session: json['session']?.toString(),
          profile: json['profile']?.toString(),
          host: json['host']?.toString(),
          error: (json['error'] ?? fields['error']) == null
              ? null
              : sanitizeLogText((json['error'] ?? fields['error']).toString()),
          seq: (json['seq'] as num?)?.toInt(),
        );
      } on Object {
        // A damaged line must not hide the remaining history.
        return null;
      }
    }
    // Host `agent.log` fmt lines: "2026-10-02T08:12:15.483Z INFO target: msg".
    // Targets may contain `::`, so the non-greedy target stops at the first
    // colon followed by whitespace.
    final match = RegExp(
      r'^(\S+)\s+(DEBUG|INFO|WARN|ERROR|TRACE)\s+(\S+?):\s+(.*)$',
    ).firstMatch(trimmed);
    if (match == null) return null;
    return AppLogRecord(
      timestamp:
          _parseTimestamp(match.group(1)) ??
          DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
      level: AppLogLevel.parse(match.group(2)),
      source: defaultSource ?? AppLogSource.hostAgent,
      target: match.group(3),
      message: sanitizeLogText(match.group(4) ?? ''),
    );
  }

  static DateTime? _parseTimestamp(Object? value) {
    if (value is String) return DateTime.tryParse(value);
    return null;
  }
}

/// Persistent JSONL records, with one rotated file per Dart isolate.
/// Reads merge independent Dart, Rust and native files for display/export.
class AppLog {
  AppLog.at(
    this._file, {
    this.maxBytes = 10 * 1024 * 1024,
    this.generations = 5,
    this.mergeSources = false,
  });

  final File _file;
  final int maxBytes;
  final int generations;
  final bool mergeSources;
  int _queued = 0;
  int _dropped = 0;
  Future<void> _pendingWrite = Future<void>.value();
  int _seq = 0;

  /// Correlation context attached to every record until changed.
  String? session;
  String? profile;
  String? host;

  static AppLog? _instance;

  /// The current isolate's log. Each producer owns its own rotation.
  static AppLog get instance => _instance ??= systemDefault();

  static AppLog systemDefault() {
    final localAppData = Platform.environment['LOCALAPPDATA'];
    final root = localAppData == null || localAppData.isEmpty
        ? Directory.current
        : Directory(localAppData);
    final logs = Directory(
      '${root.path}${Platform.pathSeparator}AetherDesk'
      '${Platform.pathSeparator}logs',
    );
    return AppLog.at(
      File(
        '${logs.path}${Platform.pathSeparator}app-$pid-${Isolate.current.hashCode}.log',
      ),
      mergeSources: true,
    );
  }

  String get path => _file.path;
  Directory get directory => _file.parent;

  void debug(
    String source,
    String message, {
    String? target,
    Object? error,
    StackTrace? stack,
  }) => _record(
    AppLogLevel.debug,
    source,
    message,
    target: target,
    error: error,
    stack: stack,
  );

  void info(
    String source,
    String message, {
    String? target,
    Object? error,
    StackTrace? stack,
  }) => _record(
    AppLogLevel.info,
    source,
    message,
    target: target,
    error: error,
    stack: stack,
  );

  void warn(
    String source,
    String message, {
    String? target,
    Object? error,
    StackTrace? stack,
  }) => _record(
    AppLogLevel.warn,
    source,
    message,
    target: target,
    error: error,
    stack: stack,
  );

  void error(
    String source,
    String message, {
    String? target,
    Object? error,
    StackTrace? stack,
  }) => _record(
    AppLogLevel.error,
    source,
    message,
    target: target,
    error: error,
    stack: stack,
  );

  void _record(
    AppLogLevel level,
    String source,
    String message, {
    String? target,
    Object? error,
    StackTrace? stack,
  }) {
    if (_queued >= 256) {
      _dropped++;
      return;
    }
    _queued++;
    final timestamp = DateTime.now();
    final sequence = ++_seq;
    final recordSession = session, recordProfile = profile, recordHost = host;
    final lost = _dropped;
    _dropped = 0;
    final trace = stack ?? (error is Error ? error.stackTrace : null);
    final operation = _pendingWrite.then(
      (_) => _write(
        AppLogRecord(
          timestamp: timestamp,
          level: level,
          source: source,
          message: sanitizeLogText(
            lost == 0
                ? message
                : "Dropped $lost log records while busy. $message",
          ),
          target: target == null ? null : sanitizeLogText(target),
          session: recordSession,
          profile: recordProfile,
          host: recordHost,
          error: error == null
              ? null
              : sanitizeLogText(trace == null ? '$error' : '$error\n$trace'),
          seq: sequence,
        ),
      ),
    );
    _pendingWrite = operation
        .catchError((Object _) {})
        .whenComplete(() => _queued--);
  }

  Future<void> _write(AppLogRecord record) async {
    await _file.parent.create(recursive: true);
    await rotateIfNeeded();
    await _file.writeAsString(
      '${jsonEncode(record.toJson())}\n',
      mode: FileMode.append,
      flush: record.level.rank >= AppLogLevel.warn.rank,
    );
  }

  /// Rotates `app.log` to `app.log.1 … app.log.N` once the size cap is hit.
  Future<void> rotateIfNeeded() async {
    if (!await _file.exists() || await _file.length() < maxBytes) return;
    await rotateNow();
  }

  Future<void> rotateNow() async {
    if (!await _file.exists()) return;
    final oldest = File('${_file.path}.$generations');
    if (await oldest.exists()) await oldest.delete();
    for (var index = generations - 1; index >= 1; index--) {
      final source = File('${_file.path}.$index');
      if (await source.exists()) {
        await source.rename('${_file.path}.${index + 1}');
      }
    }
    await _file.rename('${_file.path}.1');
  }

  /// Reads recent records (newest first) across the current file and its
  /// rotated generations.
  Future<List<AppLogRecord>> recent({
    int limit = 2000,
    int generationsToRead = 1,
  }) async {
    await _pendingWrite;
    if (limit <= 0) return [];
    final candidates = <File>[];
    if (mergeSources && await directory.exists()) {
      await for (final entry in directory.list(followLinks: false)) {
        if (entry is File &&
            RegExp(
              r'^(app|core|native)(-[0-9-]+)?\.log(\.[1-5])?$',
            ).hasMatch(entry.uri.pathSegments.last))
          candidates.add(entry);
      }
    } else {
      for (var i = 0; i <= generationsToRead; i++) {
        candidates.add(i == 0 ? _file : File('${_file.path}.$i'));
      }
    }
    final dated = <({File file, DateTime modified})>[];
    for (final file in candidates) {
      try {
        final stat = await file.stat();
        if (stat.type == FileSystemEntityType.file)
          dated.add((file: file, modified: stat.modified));
      } on FileSystemException {
        /* Concurrent rotation. */
      }
    }
    dated.sort((a, b) => b.modified.compareTo(a.modified));
    final records = <AppLogRecord>[];
    for (final entry in dated.take(24)) {
      try {
        final handle = await entry.file.open();
        List<int> bytes;
        try {
          final size = await handle.length();
          await handle.setPosition(size > 262144 ? size - 262144 : 0);
          bytes = await handle.read(262144);
        } finally {
          await handle.close();
        }
        for (final line
            in utf8.decode(bytes, allowMalformed: true).split('\n')) {
          final record = AppLogRecord.tryParse(line);
          if (record != null) records.add(record);
        }
      } on FileSystemException {
        /* Concurrent rotation must not hide other sources. */
      }
    }
    records.sort((a, b) {
      final order = b.timestamp.compareTo(a.timestamp);
      return order != 0 ? order : (b.seq ?? 0).compareTo(a.seq ?? 0);
    });
    return records.take(limit).toList();
  }
}

/// Redacts secrets and folds nothing else: stack traces must survive intact
/// (unlike the 600-char audit detail).
String sanitizeLogText(String value) {
  var text = value.replaceAll(
    RegExp(
      r'-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)',
    ),
    '<redacted private key>',
  );
  text = text.replaceAll(
    RegExp(r'AETHERDESK_AGENT_V1[^\r\n]*'),
    'AETHERDESK_AGENT_V1 <redacted>',
  );
  text = text.replaceAll(
    RegExp(r'\b(Bearer|Basic)\s+[^\s,;"\x27]+', caseSensitive: false),
    '<redacted authorization>',
  );
  text = text.replaceAllMapped(
    RegExp(
      r'''(password|passphrase|secret|token|api[_-]?key|authorization)(["']?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s,;}]+)''',
      caseSensitive: false,
    ),
    (match) => '${match.group(1)}${match.group(2)}<redacted>',
  );
  return text.length > 65536
      ? '${text.substring(0, 65536)}\n<truncated>'
      : text;
}
