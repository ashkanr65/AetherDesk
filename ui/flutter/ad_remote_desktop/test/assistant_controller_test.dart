import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:ad_remote_desktop/assistant/assistant_api.dart';
import 'package:ad_remote_desktop/assistant/assistant_controller.dart';
import 'package:ad_remote_desktop/assistant/assistant_runtime.dart';
import 'package:ad_remote_desktop/assistant/workspace_tools.dart';
import 'package:ad_remote_desktop/credential_vault.dart';
import 'package:flutter_test/flutter_test.dart';

class _Gate {
  _Gate() {
    addTearDown(() {
      if (!release.isCompleted) release.complete();
    });
  }
  final entered = Completer<void>();
  final release = Completer<void>();
  Future<void> wait() async {
    if (!entered.isCompleted) entered.complete();
    await release.future;
  }
}

class _ServerState {
  final sessions = <Map<String, Object?>>[];
  int sequence = 0;
}

class _FakeApi extends AssistantApi {
  _FakeApi(this.server)
    : super(Uri.parse('http://127.0.0.1:12345'), 'fixture-token');
  final _ServerState server;
  final requests = <({String method, String path, Object? body})>[];
  final gates = <String, _Gate>{};
  final failures = <String, Object>{};
  List<Object?> messageSnapshot = [];
  Map<String, Object?> statusSnapshot = {};
  int messageReads = 0;
  late final stream = StreamController<Map<String, dynamic>>.broadcast(
    sync: true,
    onListen: () => listeners++,
    onCancel: () => cancellations++,
  );
  int listeners = 0;
  int cancellations = 0;
  bool closed = false;

  @override
  Future<Object?> request(String method, String path, [Object? body]) async {
    if (closed) throw StateError('Request used a retired API.');
    requests.add((method: method, path: path, body: body));
    final key = '$method $path';
    final savedMessages = path.endsWith('/message')
        ? List<Object?>.of(messageSnapshot)
        : null;
    if (savedMessages != null) messageReads++;
    await gates[key]?.wait();
    final failure = failures[key];
    if (failure != null) throw failure;
    if (method == 'GET' && path == '/provider') {
      return {
        'connected': ['fixture'],
        'all': [
          {
            'id': 'fixture',
            'name': 'Fixture provider',
            'baseURL': 'https://api.example.test/v1',
            'requiresApiKey': true,
            'models': {
              'model': {'name': 'Fixture model'},
            },
          },
        ],
      };
    }
    if (method == 'POST' && path == '/session') {
      final session = <String, Object?>{
        'id': 'session-${++server.sequence}',
        'title': 'Fixture chat',
      };
      server.sessions.add(session);
      return session;
    }
    if (method == 'POST' && path == '/session/clear_history') {
      server.sessions.clear();
      return true;
    }
    if (method == 'POST' && path.endsWith('/delete')) {
      server.sessions.removeWhere(
        (session) => session['id'] == path.split('/')[2],
      );
      return true;
    }
    if (method == 'GET' && path == '/session') return server.sessions;
    if (path == '/session/status') return statusSnapshot;
    if (savedMessages != null) return savedMessages;
    if (path == '/permission' || path == '/question') return <Object?>[];
    if (path.endsWith('/prompt_async') || path.endsWith('/abort')) return null;
    throw StateError('Unexpected test API request: $method $path');
  }

  @override
  Stream<Map<String, dynamic>> events() => stream.stream;

  @override
  void close() {
    if (closed) return;
    closed = true;
    super.close();
  }
}

class _FakeRuntime extends AssistantRuntime {
  _FakeRuntime(Directory dataDirectory, this.server)
    : super(dataDirectory: dataDirectory);
  final _ServerState server;
  final apis = <_FakeApi>[];
  final configs = <Map<String, Object?>>[];
  _Gate? startGate;
  int stops = 0;

  @override
  Future<AssistantApi> start({
    required Map<String, Object?> config,
    required Directory workingDirectory,
  }) async {
    configs.add(config);
    await startGate?.wait();
    final api = _FakeApi(server);
    apis.add(api);
    return api;
  }

  @override
  Future<void> stop() async {
    stops++;
  }
}

class _FakeTools extends WorkspaceTools {
  _FakeTools(
    Directory localDirectory,
    List<AssistantRemoteHost> Function() remoteHosts,
  ) : super(localDirectory: localDirectory, remoteHosts: remoteHosts);
  int starts = 0;
  int closes = 0;
  int cancellations = 0;
  final targets = <({String local, String? remote})>[];
  _Gate? targetGate;

  @override
  Map<String, Object?> get mcpConfig => {
    'type': 'remote',
    'url': 'http://127.0.0.1:12346/mcp',
  };
  @override
  Future<void> start() async {
    starts++;
  }

  @override
  Future<void> close() async {
    closes++;
    await cancelPending();
  }

  @override
  Future<void> cancelPending() async {
    cancellations++;
  }

  @override
  Future<void> setTarget({
    required Directory localDirectory,
    String? remoteSessionId,
  }) async {
    await targetGate?.wait();
    targets.add((local: localDirectory.path, remote: remoteSessionId));
  }
}

class _FakeVault extends CredentialVault {
  final credentials = <String, StoredCredential>{};
  @override
  Future<void> delete(String id) async {
    credentials.remove(id);
  }

  @override
  Future<StoredCredential?> read(
    String profileId, {
    String? masterPassword,
  }) async => credentials[profileId];
  @override
  Future<void> store({
    required String profileId,
    required String username,
    required String secret,
    String? masterPassword,
  }) async {
    credentials[profileId] = StoredCredential(
      username: username,
      secret: secret,
    );
  }
}

Future<void> _until(bool Function() condition) async {
  final limit = DateTime.now().add(const Duration(seconds: 3));
  while (!condition()) {
    if (DateTime.now().isAfter(limit))
      fail('The controller condition did not complete.');
    await Future<void>.delayed(const Duration(milliseconds: 5));
  }
}

List<Object?> _completedMessage(String id) => [
  {
    'info': {
      'id': id,
      'role': 'assistant',
      'time': {'completed': 1},
    },
    'parts': [
      {'type': 'text', 'text': 'Completed response $id'},
    ],
  },
];

void main() {
  late Directory fixture;
  late Directory local;
  late _ServerState server;
  late _FakeRuntime runtime;
  late _FakeVault vault;
  late AssistantController controller;
  late List<AssistantRemoteHost> hosts;
  late List<_FakeTools> tools;

  AssistantController createController() => AssistantController(
    dataDirectory: fixture,
    runtime: runtime,
    vault: vault,
    remoteHosts: () => hosts,
    toolsFactory: (directory) {
      final bridge = _FakeTools(directory, () => controller.remoteHosts);
      tools.add(bridge);
      return bridge;
    },
  );

  setUp(() async {
    fixture = await Directory.systemTemp.createTemp(
      'aetherdesk-controller-test-',
    );
    local = await Directory('${fixture.path}/project').create();
    server = _ServerState();
    runtime = _FakeRuntime(fixture, server);
    vault = _FakeVault();
    tools = [];
    hosts = [
      const AssistantRemoteHost(
        id: 'ssh-1',
        identity: 'profile-account-1',
        label: 'Design server',
        directory: '/home/designer',
      ),
    ];
    controller = createController();
  });

  test(
    'removing a provider erases its saved key and backend configuration',
    () async {
      await controller.start();
      await controller.connectProvider(
        'fixture',
        'remove-me',
        baseUrl: 'https://api.example.test/v1',
      );
      expect(vault.credentials, contains('assistant/provider/fixture'));
      final previous = runtime.apis.last;
      await controller.removeProvider('fixture');
      expect(previous.closed, isTrue);
      expect(vault.credentials, isNot(contains('assistant/provider/fixture')));
      final settings =
          jsonDecode(await File('${fixture.path}/settings.json').readAsString())
              as Map;
      expect(settings['providers'], isNot(contains('fixture')));
      expect(settings['providerSettings'], isNot(contains('fixture')));
    },
  );

  test(
    'deleting active history clears saved targets and preserves provider settings',
    () async {
      await controller.start();
      await controller.newConversation();
      final first = controller.currentSessionId!;
      await controller.newConversation();
      final second = controller.currentSessionId!;
      await controller.deleteConversation(first);
      expect(controller.currentSessionId, second);
      await controller.deleteConversation(second);
      expect(controller.currentSessionId, isNull);
      expect(controller.messages, isEmpty);
      await controller.newConversation();
      await controller.clearHistory();
      expect(controller.currentSessionId, isNull);
      expect(controller.conversations, isEmpty);
      final settings =
          jsonDecode(await File('${fixture.path}/settings.json').readAsString())
              as Map;
      expect(settings['targets'], isEmpty);
      expect(settings['session'], isNull);
      expect(controller.selectedModel, isNotNull);
    },
  );

  tearDown(() async {
    for (final api in runtime.apis) {
      api.failures.clear();
    }
    await controller.shutdown();
    controller.dispose();
    for (final api in runtime.apis) {
      api.close();
      await api.stream.close();
    }
    await fixture.delete(recursive: true);
  });

  test(
    'provider retry remains busy and progress clears after completion',
    () async {
      await controller.start();
      await controller.send('hello');
      final api = runtime.apis.single;
      final id = controller.currentSessionId!;
      api.statusSnapshot[id] = {
        'type': 'retry',
        'message': 'Provider busy (HTTP 503). Retrying in 2 seconds…',
      };
      api.stream.add({
        'type': 'session.status',
        'properties': {'sessionID': id},
      });
      await _until(() => controller.progress != null);
      expect(controller.isBusy, isTrue);
      expect(controller.error, isNull);
      expect(controller.progress, contains('Retrying'));
      api.statusSnapshot[id] = {'type': 'idle'};
      api.messageSnapshot = _completedMessage('recovered-answer');
      api.stream.add({
        'type': 'message.updated',
        'properties': {'sessionID': id},
      });
      await _until(() => !controller.isBusy);
      expect(controller.progress, isNull);
      expect(controller.messages.single.text, contains('recovered-answer'));
    },
  );

  test(
    'send blocks a duplicate prompt and context changes until the response stops',
    () async {
      await controller.start();
      final api = runtime.apis.single;
      final gate = _Gate();
      api.gates['POST /session'] = gate;
      final sending = controller.send('Review these files');
      await gate.entered.future;
      expect(controller.isBusy, isTrue);
      await expectLater(controller.send('Do a second thing'), throwsStateError);
      expect(controller.newConversation, throwsStateError);
      expect(() => controller.chooseWorkspace(local.path), throwsStateError);
      gate.release.complete();
      await sending;
      expect(
        api.requests.where((request) => request.path.endsWith('/prompt_async')),
        hasLength(1),
      );
      expect(controller.isBusy, isTrue);
      await controller.abort();
      expect(controller.isBusy, isFalse);
    },
  );

  test(
    'new conversation locks out send and another workspace selection',
    () async {
      await controller.start();
      final gate = _Gate();
      runtime.apis.single.gates['POST /session'] = gate;
      final changing = controller.newConversation();
      await gate.entered.future;
      await expectLater(
        controller.send('Use the other chat'),
        throwsStateError,
      );
      expect(controller.newConversation, throwsStateError);
      expect(() => controller.chooseWorkspace(local.path), throwsStateError);
      gate.release.complete();
      await changing;
      expect(controller.isBusy, isFalse);
      expect(server.sessions, hasLength(1));
    },
  );

  test(
    'workspace changes lock the previous context until the tools target is set',
    () async {
      await controller.start();
      final gate = _Gate();
      tools.single.targetGate = gate;
      final changing = controller.chooseWorkspace(local.path);
      await gate.entered.future;
      await expectLater(controller.send('Read files'), throwsStateError);
      expect(controller.newConversation, throwsStateError);
      gate.release.complete();
      await changing;
      expect(controller.workspacePath, await local.resolveSymbolicLinks());
      expect(controller.currentSessionId, isNotNull);
      expect(controller.isBusy, isFalse);
    },
  );

  test(
    'SSH selection accepts remote paths and freezes the chosen directory',
    () async {
      await controller.start();
      await controller.chooseWorkspace(
        '/srv/chip-project',
        remoteSessionId: 'ssh-1',
      );
      expect(controller.remoteSessionId, 'ssh-1');
      expect(controller.remoteHosts.single.directory, '/srv/chip-project');
      expect(await Directory(controller.workspacePath).exists(), isTrue);
      hosts = [
        const AssistantRemoteHost(
          id: 'ssh-1',
          identity: 'profile-account-1',
          label: 'Design server',
          directory: '/srv/unrelated',
        ),
      ];
      expect(controller.remoteHosts.single.directory, '/srv/chip-project');
      expect(tools.single.remoteHosts().single.directory, '/srv/chip-project');
      await controller.send('Summarize this workspace');
      final prompt =
          runtime.apis.single.requests
                  .lastWhere(
                    (request) => request.path.endsWith('/prompt_async'),
                  )
                  .body
              as Map;
      expect(prompt['system'], contains('/srv/chip-project'));
      expect(prompt['system'], isNot(contains('/srv/unrelated')));
      await controller.abort();
    },
  );

  test(
    'saved SSH chat resumes by stable identity and restores its saved folder',
    () async {
      await controller.start();
      await controller.chooseWorkspace(
        '/srv/original-project',
        remoteSessionId: 'ssh-1',
      );
      final chat = controller.currentSessionId!;
      await controller.shutdown();
      controller.dispose();
      hosts = [
        const AssistantRemoteHost(
          id: 'unrelated',
          identity: 'different-account',
          label: 'Design server',
          directory: '/srv/wrong',
        ),
      ];
      controller = createController();
      await controller.start();
      expect(controller.currentSessionId, isNull);
      await expectLater(controller.selectConversation(chat), throwsStateError);
      expect(controller.remoteSessionId, isNull);
      hosts.add(
        const AssistantRemoteHost(
          id: 'reconnected',
          identity: 'profile-account-1',
          label: 'Renamed server',
          directory: '/srv/browser-folder',
        ),
      );
      await controller.selectConversation(chat);
      expect(controller.remoteSessionId, 'reconnected');
      expect(
        controller.remoteHosts
            .firstWhere((host) => host.id == 'reconnected')
            .directory,
        '/srv/original-project',
      );
      expect(tools.last.targets.last.remote, 'reconnected');
      expect(controller.currentSessionId, chat);
    },
  );

  test(
    'crashed backend retry retires its API, event subscription and tools',
    () async {
      await controller.start();
      await controller.newConversation();
      final oldApi = runtime.apis.single;
      final oldTools = tools.single;
      expect(oldApi.listeners, 1);
      runtime.onExit!(17);
      expect(controller.phase, AssistantPhase.failed);
      expect(oldTools.cancellations, 1);
      await controller.start();
      expect(controller.phase, AssistantPhase.ready);
      expect(oldApi.closed, isTrue);
      expect(oldApi.cancellations, 1);
      expect(oldTools.closes, 1);
      expect(tools.last.starts, 1);
      expect(runtime.apis.last.listeners, 1);
      await controller.shutdown();
      final requestCounts = runtime.apis
          .map((api) => api.requests.length)
          .toList();
      await Future<void>.delayed(const Duration(milliseconds: 2150));
      expect(
        runtime.apis.map((api) => api.requests.length).toList(),
        requestCounts,
      );
      expect(runtime.apis.last.cancellations, 1);
    },
  );

  test(
    'abort cancels workspace jobs even when the server request fails',
    () async {
      await controller.start();
      await controller.send('Run a command');
      final api = runtime.apis.single;
      api.failures['POST /session/${controller.currentSessionId}/abort'] =
          const HttpException('Backend disappeared');
      await expectLater(controller.abort(), throwsA(isA<HttpException>()));
      expect(tools.single.cancellations, 1);
      expect(tools.single.closes, 1);
      expect(api.closed, isTrue);
      expect(controller.phase, AssistantPhase.failed);
      expect(controller.isBusy, isFalse);
      expect(controller.newConversation, throwsStateError);
    },
  );

  test(
    'an old idle snapshot and idle event cannot unlock a newly sent turn',
    () async {
      await controller.start();
      await controller.newConversation();
      final chat = controller.currentSessionId!;
      final api = runtime.apis.single;
      api.messageSnapshot = _completedMessage('old-response');
      await controller.selectConversation(chat);
      expect(controller.messages.single.id, 'old-response');
      final gate = _Gate();
      api.gates['GET /session/$chat/message'] = gate;
      api.stream.add({
        'type': 'message.updated',
        'properties': {'sessionID': chat},
      });
      await gate.entered.future;
      await controller.send('Start a new response');
      expect(controller.isBusy, isTrue);
      gate.release.complete();
      // The fake transport is entirely microtask based after releasing this gate.
      await Future<void>.delayed(Duration.zero);
      expect(controller.isBusy, isTrue);
      expect(() => controller.chooseWorkspace(local.path), throwsStateError);
      final readsBeforeIdle = api.messageReads;
      api.stream.add({
        'type': 'session.idle',
        'properties': {'sessionID': chat},
      });
      await _until(() => api.messageReads > readsBeforeIdle);
      await Future<void>.delayed(Duration.zero);
      expect(controller.isBusy, isTrue);
      api.messageSnapshot = _completedMessage('new-response');
      api.stream.add({
        'type': 'message.updated',
        'properties': {'sessionID': chat},
      });
      await _until(() => !controller.isBusy);
      expect(controller.messages.single.id, 'new-response');
    },
  );

  test(
    'disconnection while creating a chat preserves its SSH identity',
    () async {
      await controller.start();
      await controller.chooseWorkspace('/srv/design', remoteSessionId: 'ssh-1');
      final api = runtime.apis.single;
      final gate = _Gate();
      api.gates['POST /session'] = gate;
      final creating = controller.newConversation();
      await gate.entered.future;
      hosts = [];
      gate.release.complete();
      await creating;
      final settings =
          jsonDecode(await File('${fixture.path}/settings.json').readAsString())
              as Map;
      final target =
          (settings['targets'] as Map)[controller.currentSessionId] as Map;
      expect(target['remoteIdentity'], 'profile-account-1');
      expect(target['remoteDirectory'], '/srv/design');
      await expectLater(controller.send('Read files'), throwsStateError);
      expect(
        api.requests.where((request) => request.path.endsWith('/prompt_async')),
        isEmpty,
      );
    },
  );

  test(
    'shutdown during startup waits and closes everything that was started',
    () async {
      final gate = _Gate();
      runtime.startGate = gate;
      final starting = controller.start();
      await gate.entered.future;
      var finished = false;
      final closing = controller.shutdown().then((_) => finished = true);
      await Future<void>.delayed(Duration.zero);
      expect(finished, isFalse);
      gate.release.complete();
      await starting;
      await closing;
      expect(controller.phase, AssistantPhase.stopped);
      expect(runtime.apis.single.closed, isTrue);
      expect(runtime.apis.single.cancellations, 1);
      expect(tools.single.closes, 1);
    },
  );

  test('shutdown still completes when a busy backend rejects Stop', () async {
    await controller.start();
    await controller.send('Run a command');
    final api = runtime.apis.single;
    api.failures['POST /session/${controller.currentSessionId}/abort'] =
        const HttpException('Backend disappeared');
    await controller.shutdown();
    expect(controller.phase, AssistantPhase.stopped);
    expect(api.closed, isTrue);
    expect(tools.single.closes, 1);
    expect(tools.single.cancellations, 1);
  });

  test(
    'provider endpoint and manual model persist without exposing the key',
    () async {
      await controller.start();
      await controller.connectProvider(
        'fixture',
        'fixture-private-key',
        baseUrl: 'https://models.example.test/v1',
        modelId: 'vendor/free-code',
      );
      final provider =
          (runtime.configs.last['provider'] as Map)['fixture'] as Map;
      expect(
        (provider['options'] as Map)['baseURL'],
        'https://models.example.test/v1',
      );
      expect(
        (provider['models'] as Map).containsKey('vendor/free-code'),
        isTrue,
      );
      final stored = await File('${fixture.path}/settings.json').readAsString();
      expect(stored, isNot(contains('fixture-private-key')));
      expect(
        controller.providerSettings('fixture')['modelId'],
        'vendor/free-code',
      );
    },
  );

  test('invalid API URLs are rejected before saving a provider key', () async {
    await controller.start();
    for (final endpoint in [
      'http://example.test/v1',
      'https://user:password@example.test/v1',
      'https://example.test/v1?key=secret',
    ]) {
      await expectLater(
        controller.connectProvider('fixture', 'secret', baseUrl: endpoint),
        throwsFormatException,
      );
    }
    expect(vault.credentials, isEmpty);
  });

  test(
    'provider credentials are kept in the vault and injected only into runtime configuration',
    () async {
      await controller.start();
      await controller.connectProvider('fixture', 'fixture-private-api-key');
      expect(
        vault.credentials['assistant/provider/fixture']!.secret,
        'fixture-private-api-key',
      );
      final config = runtime.configs.last;
      final providers = config['provider'] as Map;
      expect(
        ((providers['fixture'] as Map)['options'] as Map)['apiKey'],
        'fixture-private-api-key',
      );
      final settings = await File(
        '${fixture.path}/settings.json',
      ).readAsString();
      expect(settings, isNot(contains('fixture-private-api-key')));
      expect((jsonDecode(settings) as Map)['providers'], ['fixture']);
      expect(runtime.apis.first.closed, isTrue);
    },
  );
}
