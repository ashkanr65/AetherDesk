import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:ad_remote_desktop/assistant/workspace_tools.dart';
import 'package:crypto/crypto.dart';
import 'package:ad_remote_desktop/tools/tool_registry.dart';
import 'tool_registry_test.dart' show manifest;
import 'package:flutter_test/flutter_test.dart';

class FakeSshBackend extends AssistantToolBackend {
  final calls = <Map<String, Object?>>[];
  final files = <String, String>{'/home/project/main.txt': 'remote original'};
  String? command;
  int cancellations = 0;
  Completer<void>? gate;

  @override
  Future<void> cancelPending() async => cancellations++;

  @override
  Future<Map<String, dynamic>> remoteOperation(
    String session,
    Map<String, Object?> request,
  ) async {
    calls.add({'session': session, ...request});
    await gate?.future;
    final path = request['path']! as String;
    final canonical = path == '.'
        ? '/home/project'
        : path == '/home/project/escape'
        ? '/outside/secret'
        : path;
    Map<String, Object?> metadata() => {
      'canonicalPath': canonical,
      'kind': files.containsKey(path) ? 'file' : 'directory',
      'size': utf8.encode(files[path] ?? '').length,
      'revision': files.containsKey(path)
          ? sha256.convert(utf8.encode(files[path]!)).toString()
          : null,
    };
    switch (request['operation']) {
      case 'stat':
        return {'metadata': metadata()};
      case 'list':
        return {
          'entries': [
            {'name': 'main.txt', 'kind': 'file'},
          ],
        };
      case 'download':
        await File(request['localPath']! as String).writeAsString(files[path]!);
        return {'metadata': metadata()};
      case 'save':
      case 'createCopy':
        if (request['operation'] == 'save' &&
            request['expectedRevision'] != metadata()['revision']) {
          throw StateError('CONFLICT: remote file changed');
        }
        if (request['operation'] == 'createCopy' && files.containsKey(path)) {
          throw StateError('CONFLICT: remote file exists');
        }
        files[path] = await File(
          request['localPath']! as String,
        ).readAsString();
        return {'metadata': metadata()};
      default:
        throw StateError('Unexpected operation');
    }
  }

  @override
  Future<String> remoteCommand(String session, String value) async {
    command = value;
    final marker = RegExp(
      r'__AETHERDESK_EXIT_[a-f0-9]+__',
    ).firstMatch(value)!.group(0);
    return 'remote stderr and stdout\n${marker}7\n';
  }
}

void main() {
  late Directory directory;
  late WorkspaceTools bridge;
  late ToolRegistry registry;
  late FakeSshBackend backend;
  late List<AssistantRemoteHost> hosts;
  late HttpClient client;
  final appCalls = <Map<String, dynamic>>[];

  setUp(() async {
    directory = await Directory.systemTemp.createTemp(
      'aetherdesk-assistant-test-',
    );
    registry = ToolRegistry(
      directory: Directory('${directory.path}/installed-tools'),
    );
    backend = FakeSshBackend();
    appCalls.clear();
    hosts = [
      const AssistantRemoteHost(
        id: 'session-1',
        label: 'Connected machine',
        directory: '.',
      ),
    ];
    bridge = WorkspaceTools(
      localDirectory: directory,
      remoteHosts: () => hosts,
      backend: backend,
      registry: registry,
      appAction: (action, args, session) async {
        appCalls.add({'action': action, ...args, 'session': session});
        return {'ok': true};
      },
    );
    await bridge.start();
    client = HttpClient();
  });

  tearDown(() async {
    client.close(force: true);
    await bridge.close();
    registry.dispose();
    await directory.delete(recursive: true);
  });

  Future<(int, Map<String, dynamic>?)> request({
    String method = 'POST',
    String? auth,
    String? origin,
    String path = '/mcp',
    Map<String, Object?>? body,
  }) async {
    final outgoing = await client.openUrl(
      method,
      bridge.endpoint.replace(path: path),
    );
    outgoing.headers.set(
      HttpHeaders.authorizationHeader,
      auth ?? 'Bearer ${bridge.token}',
    );
    if (origin != null) outgoing.headers.set('origin', origin);
    outgoing.headers.contentType = ContentType.json;
    if (body != null) outgoing.write(jsonEncode(body));
    final incoming = await outgoing.close();
    final text = await utf8.decoder.bind(incoming).join();
    return (
      incoming.statusCode,
      text.isEmpty ? null : jsonDecode(text) as Map<String, dynamic>,
    );
  }

  Future<Map<String, dynamic>> call(
    String tool, [
    Map<String, Object?> arguments = const {},
  ]) async {
    final reply = await request(
      body: {
        'jsonrpc': '2.0',
        'id': 1,
        'method': 'tools/call',
        'params': {'name': tool, 'arguments': arguments},
      },
    );
    expect(reply.$1, 200);
    return reply.$2!['result'] as Map<String, dynamic>;
  }

  Map<String, dynamic> content(Map<String, dynamic> result) =>
      jsonDecode((result['content'] as List).first['text'] as String)
          as Map<String, dynamic>;

  test(
    'app opens stay inside the selected workspace and keep SSH identity',
    () async {
      await File('${directory.path}/notes.txt').writeAsString('note');
      expect(
        (await call('app_action', {
          'action': 'open_file',
          'path': '../secret',
        }))['isError'],
        true,
      );
      expect(appCalls, isEmpty);
      expect(
        (await call('app_action', {
          'action': 'open_file',
          'path': 'notes.txt',
        }))['isError'],
        isNull,
      );
      expect(
        appCalls.single['path'],
        await File('${directory.path}/notes.txt').resolveSymbolicLinks(),
      );
      expect(appCalls.single['session'], isNull);
      await bridge.setTarget(
        localDirectory: directory,
        remoteSessionId: 'session-1',
      );
      expect(
        (await call('app_action', {
          'action': 'open_file',
          'path': 'escape',
        }))['isError'],
        true,
      );
      expect(appCalls, hasLength(1));
      await call('app_action', {'action': 'open_file', 'path': 'main.txt'});
      expect(appCalls.last['path'], '/home/project/main.txt');
      expect(appCalls.last['session'], 'session-1');
    },
  );

  test(
    'MCP authenticates requests and rejects foreign origins and paths',
    () async {
      expect((await request(auth: 'Bearer incorrect')).$1, 401);
      expect((await request(origin: 'https://example.com')).$1, 403);
      expect((await request(path: '/anything')).$1, 404);
      expect((await request(method: 'GET')).$1, 405);
      expect(
        (await request(
          body: {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        )).$1,
        202,
      );
      final initialized = await request(
        body: {
          'jsonrpc': '2.0',
          'id': 'init',
          'method': 'initialize',
          'params': {'protocolVersion': '2025-11-25'},
        },
      );
      expect(initialized.$2!['result']['protocolVersion'], '2025-11-25');
      expect(bridge.mcpConfig['oauth'], false);
      expect(bridge.mcpConfig['url'], bridge.endpoint.toString());
    },
  );

  test(
    'local writes preserve concurrent changes and create new files explicitly',
    () async {
      final file = File('${directory.path}/code.txt');
      await file.writeAsString('original');
      final before = content(
        await call('workspace_read', {'path': 'code.txt'}),
      );
      expect(before['text'], 'original');
      await file.writeAsString('edited elsewhere');
      final conflict = await call('workspace_write', {
        'path': 'code.txt',
        'text': 'assistant edit',
        'expectedRevision': before['revision'],
      });
      expect(conflict['isError'], true);
      expect(await file.readAsString(), 'edited elsewhere');
      final current = content(
        await call('workspace_read', {'path': 'code.txt'}),
      );
      expect(
        (await call('workspace_write', {
          'path': 'code.txt',
          'text': 'saved',
          'expectedRevision': current['revision'],
        }))['isError'],
        isNull,
      );
      expect(await file.readAsString(), 'saved');
      expect(
        (await call('workspace_write', {
          'path': 'new.txt',
          'text': 'new content',
          'expectedRevision': 'new',
        }))['isError'],
        isNull,
      );
      expect(
        (await call('workspace_write', {
          'path': 'new.txt',
          'text': 'overwrite',
          'expectedRevision': 'new',
        }))['isError'],
        true,
      );
    },
  );

  test(
    'local file tools reject traversal, absolute paths, and oversized text',
    () async {
      for (final path in [
        '../secret',
        r'..\secret',
        r'C:\Windows\win.ini',
        '/etc/passwd',
        'code.txt:stream',
      ]) {
        expect((await call('workspace_read', {'path': path}))['isError'], true);
      }
      await File(
        '${directory.path}/large.txt',
      ).writeAsBytes(List.filled(WorkspaceTools.maxFileBytes + 1, 65));
      expect(
        (await call('workspace_read', {'path': 'large.txt'}))['isError'],
        true,
      );
      await File('${directory.path}/small.txt').writeAsString('needle\nline');
      final matches =
          content(
                await call('workspace_search', {'query': 'needle'}),
              )['matches']
              as List;
      expect(matches.length, 1);
      expect(matches.first['line'], 1);
      expect(matches.first['path'], 'small.txt');
    },
  );

  test(
    'file tools reject a directory link leaving the selected root',
    () async {
      final outside = await Directory.systemTemp.createTemp(
        'aetherdesk-assistant-outside-',
      );
      final link = Link('${directory.path}/escape');
      try {
        await File('${outside.path}/secret.txt').writeAsString('outside');
        if (Platform.isWindows) {
          final root = Platform.environment['SystemRoot'] ?? r'C:\Windows';
          final result = await Process.run('$root\\System32\\cmd.exe', [
            '/c',
            'mklink',
            '/J',
            link.path.replaceAll('/', '\\'),
            outside.path.replaceAll('/', '\\'),
          ]);
          expect(
            result.exitCode,
            0,
            reason: '${result.stdout} ${result.stderr}',
          );
        } else {
          await link.create(outside.path);
        }
        final aliasBridge = WorkspaceTools(
          localDirectory: Directory(link.path),
          remoteHosts: () => [],
        );
        try {
          await aliasBridge.start();
          expect(
            aliasBridge.localDirectory.path,
            await outside.resolveSymbolicLinks(),
          );
        } finally {
          await aliasBridge.close();
        }
        expect(
          (await call('workspace_read', {
            'path': 'escape/secret.txt',
          }))['isError'],
          true,
        );
        expect(
          (await call('workspace_write', {
            'path': 'escape/new.txt',
            'text': 'escape',
            'expectedRevision': 'new',
          }))['isError'],
          true,
        );
        expect(await File('${outside.path}/new.txt').exists(), false);
      } finally {
        if (await FileSystemEntity.type(link.path, followLinks: false) !=
            FileSystemEntityType.notFound) {
          await link.delete();
        }
        await outside.delete(recursive: true);
      }
    },
  );

  test(
    'a target cannot switch during an action and cancellation prevents follow-up downloads',
    () async {
      await bridge.setTarget(
        localDirectory: directory,
        remoteSessionId: 'session-1',
      );
      backend.gate = Completer<void>();
      final pending = call('workspace_read', {'path': 'main.txt'});
      while (backend.calls.isEmpty) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
      await expectLater(
        bridge.setTarget(localDirectory: directory),
        throwsStateError,
      );
      await bridge.cancelPending();
      backend.gate!.complete();
      expect((await pending)['isError'], true);
      expect(
        backend.calls.where((call) => call['operation'] == 'download'),
        isEmpty,
      );
      expect(backend.cancellations, 1);
      await bridge.setTarget(localDirectory: directory);
      expect(bridge.selectedRemoteSessionId, isNull);
    },
  );

  test(
    'a delayed request body cannot execute after its target changes',
    () async {
      final destination = await Directory('${directory.path}/second').create();
      await File(
        '${directory.path}/target.txt',
      ).writeAsString('original folder');
      await File('${destination.path}/target.txt').writeAsString('new folder');
      final body = jsonEncode({
        'jsonrpc': '2.0',
        'id': 41,
        'method': 'tools/call',
        'params': {
          'name': 'workspace_read',
          'arguments': {'path': 'target.txt'},
        },
      });
      final delayed = await client.postUrl(bridge.endpoint);
      delayed.headers.set(
        HttpHeaders.authorizationHeader,
        'Bearer ${bridge.token}',
      );
      delayed.headers.contentType = ContentType.json;
      delayed.contentLength = utf8.encode(body).length;
      delayed.write(body.substring(0, 10));
      await delayed.flush();
      // A second authenticated request lets the server process the first socket's
      // headers while its body is deliberately incomplete.
      expect(
        (await request(
          body: {'jsonrpc': '2.0', 'id': 42, 'method': 'ping'},
        )).$1,
        200,
      );
      await bridge.setTarget(localDirectory: destination);
      delayed.write(body.substring(10));
      final response = await delayed.close();
      final result =
          jsonDecode(await utf8.decoder.bind(response).join())['result'] as Map;
      expect(result['isError'], true);
      expect(
        (result['content'] as List).single['text'],
        contains('target changed'),
      );
      expect(
        content(await call('workspace_read', {'path': 'target.txt'}))['text'],
        'new folder',
      );
    },
  );

  test(
    'remote target uses existing session, canonical home and conflict-safe SFTP',
    () async {
      await bridge.setTarget(
        localDirectory: directory,
        remoteSessionId: 'session-1',
      );
      expect(content(await call('workspace_info'))['target'], 'SSH host');
      final read = content(await call('workspace_read', {'path': 'main.txt'}));
      expect(read['text'], 'remote original');
      expect(
        (await call('workspace_write', {
          'path': 'main.txt',
          'text': 'remote saved',
          'expectedRevision': read['revision'],
        }))['isError'],
        isNull,
      );
      expect(backend.files['/home/project/main.txt'], 'remote saved');
      expect(
        backend.calls.every((call) => call['session'] == 'session-1'),
        true,
      );
      expect(
        backend.calls
            .where((call) => call['operation'] == 'save')
            .single['expectedRevision'],
        read['revision'],
      );
      expect(
        (await call('workspace_read', {'path': 'escape'}))['isError'],
        true,
      );
      final executed = content(
        await call('workspace_command', {'command': 'printf test'}),
      );
      expect(executed['exitCode'], 7);
      expect(executed['output'], 'remote stderr and stdout');
      expect(backend.command, startsWith("cd '/home/project' &&"));
      hosts = [];
      final count = backend.calls.length;
      expect((await call('workspace_list'))['isError'], true);
      expect(backend.calls.length, count);
      await bridge.setTarget(localDirectory: directory);
      expect(
        content(await call('workspace_info'))['target'],
        'Windows computer',
      );
    },
  );

  test(
    'local commands use Windows PowerShell without a downloaded runtime',
    () async {
      if (!Platform.isWindows) return;
      final result = content(
        await call('workspace_command', {
          'command': "Write-Output 'AetherDesk test'; exit 7",
        }),
      );
      expect(result['stdout'], contains('AetherDesk test'));
      expect(result['exitCode'], 7);
      expect(result['timedOut'], false);
      expect(result['truncated'], false);
    },
  );

  test('stop terminates a running local PowerShell command', () async {
    if (!Platform.isWindows) return;
    final marker = File('${directory.path}/started.txt');
    final pending = call('workspace_command', {
      'command': "Set-Content started.txt running; Start-Sleep -Seconds 30",
    });
    final watch = Stopwatch()..start();
    while (!await marker.exists() &&
        watch.elapsed < const Duration(seconds: 8)) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(await marker.exists(), true);
    await bridge.cancelPending();
    final result = await pending.timeout(const Duration(seconds: 8));
    expect(content(result)['exitCode'], isNot(0));
    expect(watch.elapsed, lessThan(const Duration(seconds: 10)));
  });
  test(
    'MCP tool installation is persistent, separate from execution and revision protected',
    () async {
      final installed = content(
        await call('tool_install', {
          'manifest': manifest(),
          'expectedRevision': 'new',
        }),
      );
      expect(installed['installed'], true);
      expect(backend.command, isNull);
      final listed = content(await call('tool_list'))['tools'] as List;
      expect(listed.single['id'], 'test-tool');
      final read = content(await call('tool_read', {'id': 'test-tool'}));
      expect(read['manifest']['script'], 'Write-Output test');
      expect(read['revision'], installed['revision']);
      expect(
        (await call('tool_install', {
          'manifest': manifest(),
          'expectedRevision': 'new',
        }))['isError'],
        true,
      );
      expect(
        (await call('tool_run', {
          'id': 'test-tool',
          'expectedRevision': 'old',
          'arguments': {'message': 'input'},
        }))['isError'],
        true,
      );
    },
  );

  test(
    'saved tools enforce target, revision and literal SSH parameters',
    () async {
      final tool = await registry.install(
        manifest(target: 'ssh', script: r'printf "%s" "$AETHER_TOOL_message"'),
        expectedRevision: 'new',
      );
      await expectLater(
        bridge.executeTool(
          tool.id,
          expectedRevision: tool.revision,
          arguments: {'message': 'value'},
        ),
        throwsStateError,
      );
      expect(backend.command, isNull);
      await bridge.setTarget(
        localDirectory: directory,
        remoteSessionId: 'session-1',
      );
      final result =
          await bridge.executeTool(
                tool.id,
                expectedRevision: tool.revision,
                arguments: {'message': "text'; pwd"},
              )
              as Map;
      expect(result['exitCode'], 7);
      expect(backend.command, contains('AETHER_TOOL_message='));
      expect(backend.command, startsWith("cd '/home/project' &&"));
      await registry.install(
        manifest(target: 'ssh', script: 'changed'),
        expectedRevision: tool.revision,
      );
      final oldCommand = backend.command;
      await expectLater(
        bridge.executeTool(
          tool.id,
          expectedRevision: tool.revision,
          arguments: {'message': 'value'},
        ),
        throwsStateError,
      );
      expect(backend.command, oldCommand);
      hosts = [];
      await expectLater(
        bridge.executeTool(tool.id, expectedRevision: tool.revision),
        throwsStateError,
      );
    },
  );

  test(
    'Windows tool parameters remain literal when executed by PowerShell',
    () async {
      if (!Platform.isWindows) return;
      const input = "text'; Set-Content injection.txt bad; '";
      final tool = await registry.install(
        manifest(script: r'Write-Output $env:AETHER_TOOL_message'),
        expectedRevision: 'new',
      );
      final result =
          await bridge.executeTool(
                tool.id,
                expectedRevision: tool.revision,
                arguments: {'message': input},
              )
              as Map;
      expect(result['stdout'], contains(input));
      expect(result['exitCode'], 0);
      expect(await File('${directory.path}/injection.txt').exists(), false);
    },
  );
}
