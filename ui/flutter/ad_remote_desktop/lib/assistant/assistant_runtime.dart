import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:math';

import '../app_log.dart';

import 'package:crypto/crypto.dart';

import 'assistant_api.dart';

Directory assistantDataDirectory() => Directory(
  '${Platform.environment['LOCALAPPDATA'] ?? Directory.systemTemp.path}${Platform.pathSeparator}AetherDesk${Platform.pathSeparator}assistant',
);

class AssistantRuntime {
  AssistantRuntime({Directory? applicationDirectory, Directory? dataDirectory})
    : applicationDirectory =
          applicationDirectory ?? File(Platform.resolvedExecutable).parent,
      dataDirectory = dataDirectory ?? assistantDataDirectory();

  final Directory applicationDirectory;
  final Directory dataDirectory;
  Process? _process;
  AssistantApi? _api;
  Future<AssistantApi>? _starting;
  bool _stopping = false;
  void Function(int code)? onExit;

  Future<AssistantApi> start({
    required Map<String, Object?> config,
    required Directory workingDirectory,
  }) {
    return _starting ??= _start(
      config,
      workingDirectory,
    ).whenComplete(() => _starting = null);
  }

  Future<AssistantApi> _start(
    Map<String, Object?> config,
    Directory directory,
  ) async {
    if (_process != null) throw StateError('The assistant is already running.');
    _stopping = false;
    final root = Directory('${applicationDirectory.path}/ai/assistant');
    final executable = File('${root.path}/python/python.exe');
    final manifestFile = File('${root.path}/bundle-info.json');
    if (!Platform.isWindows ||
        !await executable.exists() ||
        !await manifestFile.exists()) {
      throw StateError(
        'The bundled AI Assistant is missing. Use the complete AetherDesk installer or extract the entire portable ZIP.',
      );
    }
    final manifest = jsonDecode(await manifestFile.readAsString()) as Map;
    if (manifest['schemaVersion'] != 1 ||
        manifest['engine'] != 'langgraph' ||
        manifest['executable'] != 'python/python.exe' ||
        manifest['entrypoint'] != 'server.py') {
      throw const FormatException(
        'The bundled AI Assistant manifest is invalid.',
      );
    }
    await _verifyBundleFiles(root.path, manifest['files'] as List);
    await dataDirectory.create(recursive: true);
    await directory.create(recursive: true);
    final socket = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = socket.port;
    await socket.close();
    final random = Random.secure();
    final password = base64UrlEncode(
      List<int>.generate(32, (_) => random.nextInt(256)),
    );
    final system = Platform.environment['SystemRoot'] ?? r'C:\Windows';
    // The backend gets an isolated home and config. It cannot load arbitrary
    // project plugins, inherit provider credentials, or install extra tools.
    final environment = <String, String>{
      for (final name in [
        'SystemRoot',
        'WINDIR',
        'TEMP',
        'TMP',
        'COMSPEC',
        'PATHEXT',
      ])
        if (Platform.environment[name] != null)
          name: Platform.environment[name]!,
      'PATH': '$system\\System32',
      'HOME': dataDirectory.path,
      'USERPROFILE': dataDirectory.path,
      'LOCALAPPDATA': dataDirectory.path,
      'APPDATA': dataDirectory.path,
      'AETHERDESK_ASSISTANT_TOKEN': password,
      'AETHERDESK_ASSISTANT_CONFIG': jsonEncode(config),
      'AETHERDESK_ASSISTANT_DATA': '${dataDirectory.path}/langgraph',
      'LANGSMITH_TRACING': 'false',
      'LANGCHAIN_TRACING_V2': 'false',
    };
    final process = await Process.start(
      executable.path,
      ['-I', '-B', '${root.path}/server.py', '--port', '$port'],
      workingDirectory: directory.path,
      environment: environment,
      includeParentEnvironment: false,
    );
    _process = process;
    unawaited(_captureOutput(process.stdout));
    unawaited(_captureOutput(process.stderr, isError: true));
    int? exitCode;
    unawaited(
      process.exitCode.then((code) {
        exitCode = code;
        if (identical(_process, process)) {
          _process = null;
          if (!_stopping) onExit?.call(code);
        }
      }),
    );
    final api = AssistantApi(Uri.parse('http://127.0.0.1:$port'), password);
    _api = api;
    final deadline = DateTime.now().add(const Duration(seconds: 30));
    try {
      while (DateTime.now().isBefore(deadline)) {
        if (exitCode != null)
          throw StateError(
            'The bundled AI Assistant stopped during startup (code $exitCode).',
          );
        try {
          final health = await api.request('GET', '/global/health');
          if (health is Map && health['healthy'] == true) {
            if (health['engine'] != 'langgraph' ||
                health['version'] != manifest['version'])
              throw const FormatException(
                'The AI Assistant version does not match its package.',
              );
            return api;
          }
        } on SocketException {
          // Wait for our child to bind its private port.
        } on HttpException {
          // A port conflict never becomes an authenticated connection.
        }
        await Future<void>.delayed(const Duration(milliseconds: 150));
      }
      throw TimeoutException('The bundled AI Assistant did not become ready.');
    } catch (_) {
      await stop();
      rethrow;
    }
  }

  /// Mirrors the Python service's stdout/stderr into the application log
  /// instead of discarding it, including service tracebacks
  /// (log_feature.md D).
  Future<void> _captureOutput(
    Stream<List<int>> stream, {
    bool isError = false,
  }) async {
    final lines = stream
        .transform(const Utf8Decoder(allowMalformed: true))
        .transform(const LineSplitter());
    await for (final line in lines) {
      if (line.trim().isEmpty) continue;
      if (isError) {
        AppLog.instance.warn(AppLogSource.assistant, line);
      } else {
        AppLog.instance.info(AppLogSource.assistant, line);
      }
    }
  }

  Future<void> stop() async {
    _stopping = true;
    _api?.close();
    _api = null;
    final process = _process;
    _process = null;
    if (process != null) {
      process.kill();
      try {
        await process.exitCode.timeout(const Duration(seconds: 5));
      } on TimeoutException {
        process.kill(ProcessSignal.sigkill);
        await process.exitCode.timeout(const Duration(seconds: 3));
      }
    }
  }
}

// Keep this closure outside the runtime instance: Dart closures in _start can
// share a context containing unsendable Process/Future objects.
Future<void> _verifyBundleFiles(
  String root,
  List<dynamic> entries,
) => Isolate.run(() async {
  final paths = <String>{};
  for (final item in entries) {
    if (item is! Map)
      throw const FormatException('Invalid assistant manifest.');
    final path = item['path'];
    if (path is! String ||
        path.isEmpty ||
        path.contains('\\') ||
        path.contains(':') ||
        path.startsWith('/') ||
        path
            .split('/')
            .any((part) => part.isEmpty || part == '.' || part == '..') ||
        !paths.add(path.toLowerCase())) {
      throw const FormatException('Invalid assistant package path.');
    }
    final binary = File('$root/$path');
    if (!await binary.exists() ||
        await binary.length() != item['size'] ||
        (await sha256.bind(binary.openRead()).first).toString() !=
            item['sha256']) {
      throw const FormatException(
        'The bundled AI Assistant is damaged. Extract a fresh complete AetherDesk package.',
      );
    }
  }
  if (!paths.containsAll({'python/python.exe', 'server.py'})) {
    throw const FormatException('The bundled AI Assistant is incomplete.');
  }
});
