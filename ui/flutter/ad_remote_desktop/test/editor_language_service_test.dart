import 'package:flutter_test/flutter_test.dart';
import 'package:ad_remote_desktop/editor/editor_language_service.dart';

class FakeLanguageTransport implements EditorLanguageTransport {
  final sent = <Map<String, Object?>>[];
  final queued = <Map<String, dynamic>>[];
  bool stopped = false;
  bool failSend = false;
  Map<String, Object?>? config;
  @override
  Future<String> start(String session, Map<String, Object?> value) async {
    expect(session, 'session');
    config = value;
    return 'server';
  }

  @override
  Future<void> send(String id, Map<String, Object?> message) async {
    expect(id, 'server');
    if (failSend) throw StateError('Disconnected');
    sent.add(message);
    if (message['id'] != null && message['method'] != null) {
      queued.add({
        'jsonrpc': '2.0',
        'id': message['id'],
        'result': message['method'] == 'initialize'
            ? {
                'capabilities': {'completionProvider': {}},
              }
            : {'ok': true},
      });
    }
  }

  @override
  Future<Map<String, dynamic>> poll(String id) async {
    final messages = [...queued];
    queued.clear();
    return {'state': stopped ? 'stopped' : 'running', 'messages': messages};
  }

  @override
  Future<void> stop(String id) async {
    stopped = true;
  }
}

void main() {
  test(
    'initializes over explicit SSH configuration and preserves host document URIs',
    () async {
      final transport = FakeLanguageTransport();
      final service = EditorLanguageService(
        sessionId: 'session',
        executable: '/usr/bin/clangd',
        arguments: ['--background-index'],
        workingDirectory: '/projects/a b',
        transport: transport,
      );
      final notifications = <Map<String, dynamic>>[];
      final listener = service.notifications.listen(notifications.add);
      await service.openDocument(
        path: '/projects/a b/ñ.cpp',
        languageId: 'cpp',
        text: 'int main() {}',
      );
      expect(transport.config?['executable'], '/usr/bin/clangd');
      expect(
        service.serverCapabilities.containsKey('completionProvider'),
        true,
      );
      final open = transport.sent.firstWhere(
        (e) => e['method'] == 'textDocument/didOpen',
      );
      expect(
        ((open['params'] as Map)['textDocument'] as Map)['uri'],
        'file:///projects/a%20b/%C3%B1.cpp',
      );
      await service.changeDocument(
        path: '/projects/a b/ñ.cpp',
        text: 'int main() {return 0;}',
        version: 2,
      );
      await service.changeDocument(
        path: '/projects/a b/ñ.cpp',
        text: 'old',
        version: 1,
      );
      expect(
        transport.sent
            .where((e) => e['method'] == 'textDocument/didChange')
            .length,
        1,
      );
      transport.queued.add({
        'jsonrpc': '2.0',
        'method': 'textDocument/publishDiagnostics',
        'params': {'uri': 'file:///projects/a.cpp', 'diagnostics': []},
      });
      final result = await service.request('textDocument/completion', {
        'textDocument': {'uri': 'file:///projects/a.cpp'},
        'position': {'line': 0, 'character': 3},
      });
      expect(result, {'ok': true});
      expect(notifications.single['method'], 'textDocument/publishDiagnostics');
      await service.closeDocument('/projects/a b/ñ.cpp');
      await service.dispose();
      expect(transport.stopped, true);
      expect(transport.sent.any((e) => e['method'] == 'shutdown'), true);
      await listener.cancel();
    },
  );

  test('failed native sends fail the request and dispose safely', () async {
    final transport = FakeLanguageTransport();
    final service = EditorLanguageService(
      sessionId: 'session',
      executable: 'server',
      transport: transport,
    );
    await service.start();
    transport.failSend = true;
    await expectLater(
      service.request('textDocument/hover', {}),
      throwsStateError,
    );
    await service.dispose();
    expect(transport.stopped, true);
  });
}
