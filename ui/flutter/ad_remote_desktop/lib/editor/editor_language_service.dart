import 'dart:async';

import '../native_bridge.dart';

/// The server executable and arguments are explicitly configured by the user.
/// Each instance uses a separate authenticated SSH channel, never terminal stdin.
class EditorLanguageService {
  EditorLanguageService({
    required this.sessionId,
    required this.executable,
    this.arguments = const [],
    this.workingDirectory,
    EditorLanguageTransport? transport,
  }) : _transport = transport ?? const NativeEditorLanguageTransport();

  final String sessionId;
  final String executable;
  final List<String> arguments;
  final String? workingDirectory;
  final EditorLanguageTransport _transport;
  final _notifications = StreamController<Map<String, dynamic>>.broadcast();
  final _pending = <int, Completer<Object?>>{};
  final _documents = <String, int>{};
  Future<void>? _starting;
  String? _id;
  Timer? _timer;
  int _nextId = 0;
  bool _polling = false;
  bool _disposed = false;
  bool _disposing = false;
  Object? _failure;
  Map<String, dynamic> serverCapabilities = const {};

  Stream<Map<String, dynamic>> get notifications => _notifications.stream;

  static String documentUri(String path) =>
      Uri.file(path, windows: false).toString();

  Future<void> start() {
    if (_disposed || _disposing)
      return Future.error(StateError('Language server is closed'));
    if (_failure != null) return Future.error(_failure!);
    return _starting ??= _start();
  }

  Future<void> _start() async {
    final id = await _transport.start(sessionId, {
      'executable': executable,
      'arguments': arguments,
      if (workingDirectory != null) 'workingDirectory': workingDirectory,
    });
    if (_disposed || _disposing) {
      await _transport.stop(id);
      throw StateError('Language server is closed');
    }
    _id = id;
    _timer = Timer.periodic(const Duration(milliseconds: 150), (_) => _poll());
    try {
      final result = await _sendRequest('initialize', {
        'processId': null,
        'clientInfo': {'name': 'AetherDesk'},
        'rootUri': workingDirectory == null
            ? null
            : documentUri(workingDirectory!),
        'workspaceFolders': _workspaceFolders(),
        'capabilities': {
          'general': {
            'positionEncodings': ['utf-16'],
          },
          'workspace': {'configuration': true, 'workspaceFolders': true},
          'textDocument': {
            'synchronization': {'didSave': true, 'dynamicRegistration': false},
            'completion': {
              'completionItem': {'snippetSupport': false},
            },
            'publishDiagnostics': {
              'relatedInformation': true,
              'versionSupport': true,
            },
          },
        },
      });
      if (result is Map && result['capabilities'] is Map) {
        serverCapabilities = Map<String, dynamic>.from(
          result['capabilities'] as Map,
        );
      }
      await _sendNotification('initialized', {});
    } catch (_) {
      _timer?.cancel();
      await _transport.stop(id);
      _id = null;
      rethrow;
    }
  }

  List<Map<String, String>>? _workspaceFolders() => workingDirectory == null
      ? null
      : [
          {
            'uri': documentUri(workingDirectory!),
            'name':
                workingDirectory!
                    .split('/')
                    .where((s) => s.isNotEmpty)
                    .lastOrNull ??
                '/',
          },
        ];

  Future<Object?> request(String method, Map<String, Object?> params) async {
    await start();
    return _sendRequest(method, params);
  }

  Future<void> notify(String method, Map<String, Object?> params) async {
    await start();
    await _sendNotification(method, params);
  }

  Future<Object?> _sendRequest(
    String method,
    Map<String, Object?> params,
  ) async {
    final server = _id;
    if (server == null) throw StateError('Language server is not running');
    if (_pending.length >= 64)
      throw StateError('Too many pending language server requests');
    final id = ++_nextId;
    final response = Completer<Object?>();
    _pending[id] = response;
    // Attach the timeout before sending so an immediate server exit cannot
    // complete an unobserved error future while the native send is pending.
    final awaited = response.future.timeout(
      const Duration(seconds: 20),
      onTimeout: () {
        unawaited(
          _transport
              .send(server, {
                'jsonrpc': '2.0',
                'method': r'$/cancelRequest',
                'params': {'id': id},
              })
              .catchError((Object _) {}),
        );
        throw TimeoutException('Language server did not answer $method');
      },
    );
    try {
      final results = await Future.wait<Object?>([
        _transport
            .send(server, {
              'jsonrpc': '2.0',
              'id': id,
              'method': method,
              'params': params,
            })
            .then<Object?>((_) => null)
            .catchError((Object error, StackTrace stack) {
              if (!response.isCompleted) response.completeError(error, stack);
              return null;
            }),
        awaited,
      ]);
      return results[1];
    } finally {
      _pending.remove(id);
    }
  }

  Future<void> _sendNotification(
    String method,
    Map<String, Object?> params,
  ) async {
    final id = _id;
    if (id == null) throw StateError('Language server is not running');
    await _transport.send(id, {
      'jsonrpc': '2.0',
      'method': method,
      'params': params,
    });
  }

  Future<void> openDocument({
    required String path,
    required String languageId,
    required String text,
    int version = 1,
  }) async {
    await start();
    final uri = documentUri(path);
    if (_documents.containsKey(uri)) return;
    await _sendNotification('textDocument/didOpen', {
      'textDocument': {
        'uri': uri,
        'languageId': languageId,
        'version': version,
        'text': text,
      },
    });
    _documents[uri] = version;
  }

  Future<void> changeDocument({
    required String path,
    required String text,
    required int version,
  }) async {
    final uri = documentUri(path);
    final previous = _documents[uri];
    if (previous == null)
      throw StateError('Open the document before changing it');
    if (version <= previous) return;
    await notify('textDocument/didChange', {
      'textDocument': {'uri': uri, 'version': version},
      'contentChanges': [
        {'text': text},
      ],
    });
    _documents[uri] = version;
  }

  Future<void> saveDocument(String path) => notify('textDocument/didSave', {
    'textDocument': {'uri': documentUri(path)},
  });

  Future<void> closeDocument(String path) async {
    final uri = documentUri(path);
    if (_documents.remove(uri) == null || _id == null || _disposed) return;
    await _sendNotification('textDocument/didClose', {
      'textDocument': {'uri': uri},
    });
  }

  Future<void> _poll() async {
    final id = _id;
    if (_polling || _disposed || id == null) return;
    _polling = true;
    try {
      final value = await _transport.poll(id);
      if (_disposed) return;
      for (final item in (value['messages'] as List? ?? const [])) {
        if (item is! Map) continue;
        final message = Map<String, dynamic>.from(item);
        if (message['method'] is String) {
          if (message.containsKey('id')) {
            await _answerServerRequest(id, message);
          } else {
            _notifications.add(message);
          }
        } else if (message['id'] is int) {
          final pending = _pending[message['id']];
          if (pending != null && !pending.isCompleted) {
            if (message['error'] != null) {
              pending.completeError(
                StateError('Language server: ${message['error']}'),
              );
            } else {
              pending.complete(message['result']);
            }
          }
        }
      }
      if (value['state'] != 'running')
        throw StateError(
          value['error']?.toString() ?? 'Language server stopped',
        );
    } catch (error) {
      _failure = error;
      _timer?.cancel();
      for (final pending in _pending.values) {
        if (!pending.isCompleted) pending.completeError(error);
      }
      if (!_disposed)
        _notifications.add({
          'method': 'aetherdesk/languageServerError',
          'params': {'message': error.toString()},
        });
    } finally {
      _polling = false;
    }
  }

  Future<void> _answerServerRequest(
    String id,
    Map<String, dynamic> message,
  ) async {
    final method = message['method'];
    Object? result;
    switch (method) {
      case 'workspace/configuration':
        final params = message['params'];
        result = params is Map && params['items'] is List
            ? List<Object?>.filled((params['items'] as List).length, null)
            : <Object?>[];
      case 'workspace/workspaceFolders':
        result = _workspaceFolders();
      case 'workspace/applyEdit':
        result = {
          'applied': false,
          'failureReason': 'Apply workspace edits from the editor explicitly.',
        };
      case 'window/showMessageRequest':
      case 'client/registerCapability':
      case 'client/unregisterCapability':
        result = null;
      default:
        await _transport.send(id, {
          'jsonrpc': '2.0',
          'id': message['id'],
          'error': {'code': -32601, 'message': 'Client method not supported'},
        });
        return;
    }
    await _transport.send(id, {
      'jsonrpc': '2.0',
      'id': message['id'],
      'result': result,
    });
  }

  Future<void> dispose() async {
    if (_disposed || _disposing) return;
    _disposing = true;
    final id = _id;
    if (id != null) {
      try {
        await _sendRequest('shutdown', {}).timeout(const Duration(seconds: 2));
        await _sendNotification('exit', {});
      } catch (_) {
        // Closing the dedicated channel also stops a stalled server.
      }
      await _transport.stop(id);
    }
    _disposed = true;
    _timer?.cancel();
    _id = null;
    for (final pending in _pending.values) {
      if (!pending.isCompleted)
        pending.completeError(StateError('Language server is closed'));
    }
    _pending.clear();
    _documents.clear();
    await _notifications.close();
  }
}

abstract interface class EditorLanguageTransport {
  Future<String> start(String session, Map<String, Object?> config);
  Future<void> send(String id, Map<String, Object?> message);
  Future<Map<String, dynamic>> poll(String id);
  Future<void> stop(String id);
}

class NativeEditorLanguageTransport implements EditorLanguageTransport {
  const NativeEditorLanguageTransport();
  @override
  Future<String> start(String session, Map<String, Object?> config) =>
      NativeBridge.startLanguageServer(session, config);
  @override
  Future<void> send(String id, Map<String, Object?> message) =>
      NativeBridge.sendLanguageServer(id, message);
  @override
  Future<Map<String, dynamic>> poll(String id) =>
      NativeBridge.pollLanguageServer(id);
  @override
  Future<void> stop(String id) => NativeBridge.stopLanguageServer(id);
}
