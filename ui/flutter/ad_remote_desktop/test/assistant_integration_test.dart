import 'dart:convert';
import 'dart:io';

import 'package:ad_remote_desktop/assistant/assistant_api.dart';
import 'package:ad_remote_desktop/assistant/assistant_controller.dart';
import 'package:ad_remote_desktop/assistant/assistant_runtime.dart';
import 'package:ad_remote_desktop/assistant/workspace_tools.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ad_remote_desktop/tools/tool_registry.dart';

import 'assistant_workspace_tools_test.dart' show FakeSshBackend;

class _TestModelRuntime extends AssistantRuntime {
  _TestModelRuntime({
    required Directory applicationDirectory,
    required Directory dataDirectory,
    required this.modelUrl,
  }) : super(
         applicationDirectory: applicationDirectory,
         dataDirectory: dataDirectory,
       );
  final String modelUrl;

  @override
  Future<AssistantApi> start({
    required Map<String, Object?> config,
    required Directory workingDirectory,
  }) {
    return super.start(
      config: {
        ...config,
        'enabled_providers': ['aetherdesk-test'],
        'provider': {
          'aetherdesk-test': {
            'name': 'Local integration model',
            'options': {'baseURL': modelUrl, 'apiKey': 'local-test-only'},
            'models': {
              'test-model': {
                'name': 'Test model',
                'limit': {'context': 32000, 'output': 2048},
              },
            },
          },
        },
      },
      workingDirectory: workingDirectory,
    );
  }
}

Future<void> _until(
  bool Function() done,
  AssistantController controller,
) async {
  final deadline = DateTime.now().add(const Duration(seconds: 30));
  while (!done()) {
    if (DateTime.now().isAfter(deadline)) {
      fail(
        'Assistant condition timed out: phase=${controller.phase}, busy=${controller.isBusy}, error=${controller.error}, messages=${controller.messages.map((m) => '${m.text} ${m.error} ${m.tools.map((t) => '${t.name}:${t.status}:${t.output}')}').toList()}',
      );
    }
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }
}

void main() {
  final package = Platform.environment['AETHERDESK_ASSISTANT_PACKAGE'];
  test(
    'bundled LangGraph chats and gates Windows and SSH actions without installed runtimes',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-assistant-integration-',
      );
      final workspace = await Directory('${directory.path}/workspace').create();
      await File(
        '${workspace.path}/notes.txt',
      ).writeAsString('Selected workspace contents.');
      final model = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final requests = <Map<String, dynamic>>[];
      final toolNames = <String>{};
      final registry = ToolRegistry(
        directory: Directory('${directory.path}/tools'),
      );
      String? installedRevision;
      const manifest = {
        'schemaVersion': 1,
        'id': 'integration-tool',
        'title': 'Integration tool',
        'description': 'A saved tool created by the assistant.',
        'version': '1.0.0',
        'target': 'windows',
        'script':
            "Set-Content -LiteralPath 'tool-marker.txt' -Value 'Created by saved tool'; Write-Output 'Saved tool finished.'",
        'parameters': <Object>[],
      };
      model.listen((request) async {
        try {
          if (request.method == 'GET' && request.uri.path == '/v1/models') {
            request.response.headers.contentType = ContentType.json;
            request.response.write(
              jsonEncode({
                'data': [
                  {'id': 'test-model', 'name': 'Test model'},
                ],
              }),
            );
            return;
          }
          final body = Map<String, dynamic>.from(
            jsonDecode(await utf8.decoder.bind(request).join()) as Map,
          );
          requests.add(body);
          for (final tool in (body['tools'] as List? ?? []).whereType<Map>()) {
            toolNames.add((tool['function'] as Map)['name'] as String);
          }
          final messages = (body['messages'] as List).whereType<Map>().toList();
          final lastUser = messages.lastIndexWhere(
            (message) => message['role'] == 'user',
          );
          final prompt = messages[lastUser]['content'].toString();
          final afterTool = messages
              .skip(lastUser + 1)
              .any((message) => message['role'] == 'tool');
          Map<String, Object?> delta;
          String finish;
          if (!afterTool &&
              (prompt.contains('use read tool') ||
                  prompt.contains('use write tool') ||
                  prompt.contains('use command tool') ||
                  prompt.contains('install saved tool') ||
                  prompt.contains('run saved tool'))) {
            final install = prompt.contains('install saved tool');
            final runSaved = prompt.contains('run saved tool');
            final write = prompt.contains('use write tool');
            final command = prompt.contains('use command tool');
            delta = {
              'role': 'assistant',
              'tool_calls': [
                {
                  'index': 0,
                  'id': 'call_${requests.length}',
                  'type': 'function',
                  'function': {
                    'name': install
                        ? 'aetherdesk_tool_install'
                        : runSaved
                        ? 'aetherdesk_tool_run'
                        : command
                        ? 'aetherdesk_workspace_command'
                        : write
                        ? 'aetherdesk_workspace_write'
                        : 'aetherdesk_workspace_read',
                    'arguments': jsonEncode(
                      install
                          ? {'manifest': manifest, 'expectedRevision': 'new'}
                          : runSaved
                          ? {
                              'id': 'integration-tool',
                              'expectedRevision': installedRevision,
                              'arguments': <String, String>{},
                            }
                          : command
                          ? {'command': 'printf integration-test'}
                          : write
                          ? {
                              'path': 'created.txt',
                              'text': prompt.contains('SSH')
                                  ? 'Remote approved edit.'
                                  : 'Approved edit.',
                              'expectedRevision': 'new',
                            }
                          : {'path': 'notes.txt'},
                    ),
                  },
                },
              ],
            };
            finish = 'tool_calls';
          } else {
            delta = {
              'role': 'assistant',
              'content': afterTool
                  ? 'Tool complete.'
                  : 'Bundled LangGraph chat works.',
            };
            finish = 'stop';
          }
          request.response.headers.contentType = ContentType(
            'text',
            'event-stream',
          );
          void event(Map<String, Object?> change, Object? reason) {
            request.response.write(
              'data: ${jsonEncode({
                'id': 'chatcmpl-${requests.length}',
                'object': 'chat.completion.chunk',
                'created': 1,
                'model': 'test-model',
                'choices': [
                  {'index': 0, 'delta': change, 'finish_reason': reason},
                ],
              })}\n\n',
            );
          }

          event(delta, null);
          event({}, finish);
          request.response.write('data: [DONE]\n\n');
        } finally {
          await request.response.close();
        }
      });
      final runtime = _TestModelRuntime(
        applicationDirectory: Directory(package!),
        dataDirectory: Directory('${directory.path}/state'),
        modelUrl: 'http://127.0.0.1:${model.port}/v1',
      );
      final ssh = FakeSshBackend()
        ..files['/home/project/notes.txt'] = 'Contents from the SSH host.';
      final hosts = [
        const AssistantRemoteHost(
          id: 'integration-ssh',
          identity: 'test-user@integration-host:22',
          label: 'Integration SSH host',
          directory: '.',
        ),
      ];
      late AssistantController controller;
      controller = AssistantController(
        runtime: runtime,
        remoteHosts: () => hosts,
        toolsFactory: (directory) => WorkspaceTools(
          localDirectory: directory,
          remoteHosts: () => controller.remoteHosts,
          backend: ssh,
          registry: registry,
        ),
      );
      try {
        await controller.start();
        expect(
          controller.phase,
          AssistantPhase.ready,
          reason: controller.error,
        );
        expect(controller.selectedModel?.providerId, 'aetherdesk-test');
        await controller.chooseWorkspace(workspace.path);
        await controller.send('Verify plain chat.');
        await _until(
          () =>
              !controller.isBusy &&
              controller.messages.any(
                (m) => m.text.contains('Bundled LangGraph chat works.'),
              ),
          controller,
        );
        await controller.send('Please use read tool on notes.txt.');
        await _until(
          () =>
              !controller.isBusy &&
              controller.messages.any(
                (m) => m.tools.any(
                  (t) => t.output.contains('Selected workspace contents.'),
                ),
              ),
          controller,
        );
        expect(toolNames, contains('aetherdesk_workspace_read'));
        expect(toolNames, isNot(contains('bash')));
        expect(toolNames, isNot(contains('glob')));

        await controller.send('Please use write tool to create created.txt.');
        await _until(() => controller.approvals.isNotEmpty, controller);
        expect(await File('${workspace.path}/created.txt').exists(), isFalse);
        await controller.replyPermission(controller.approvals.single.id, false);
        await _until(() => !controller.isBusy, controller);
        expect(await File('${workspace.path}/created.txt').exists(), isFalse);

        await controller.send(
          'Please use write tool to create created.txt now.',
        );
        await _until(() => controller.approvals.isNotEmpty, controller);
        await controller.replyPermission(controller.approvals.single.id, true);
        await _until(
          () =>
              !controller.isBusy &&
              File('${workspace.path}/created.txt').existsSync(),
          controller,
        );
        expect(
          await File('${workspace.path}/created.txt').readAsString(),
          'Approved edit.',
        );
        await controller.send('Please install saved tool in my Tools menu.');
        await _until(() => controller.approvals.isNotEmpty, controller);
        expect(registry.tools, isEmpty);
        expect(
          controller.approvals.single.metadata.toString(),
          contains('tool-marker.txt'),
        );
        await controller.replyPermission(controller.approvals.single.id, true);
        await _until(
          () => !controller.isBusy && registry.tools.isNotEmpty,
          controller,
        );
        final installed = await registry.read('integration-tool');
        installedRevision = installed.revision;
        final markerFile = File('${workspace.path}/tool-marker.txt');
        expect(
          await markerFile.exists(),
          isFalse,
          reason: 'Installation must not execute the tool.',
        );
        await controller.send('Please run saved tool.');
        await _until(() => controller.approvals.isNotEmpty, controller);
        expect(
          controller.approvals.single.metadata.toString(),
          contains(manifest['script']),
        );
        expect(await markerFile.exists(), isFalse);
        await controller.replyPermission(controller.approvals.single.id, true);
        await _until(
          () => !controller.isBusy && markerFile.existsSync(),
          controller,
        );
        expect(
          (await markerFile.readAsString()).trim(),
          'Created by saved tool',
        );
        expect(
          controller.messages.any(
            (m) =>
                m.tools.any((t) => t.output.contains('Saved tool finished.')),
          ),
          isTrue,
        );

        // LangGraph remains a local process; its scoped MCP tools must route
        // POSIX paths through the selected existing SSH session.
        await controller.chooseWorkspace(
          '/home/project',
          remoteSessionId: 'integration-ssh',
        );
        expect(controller.workspacePath, isNot('/home/project'));
        await controller.send('Please use read tool on SSH notes.txt.');
        await _until(
          () =>
              !controller.isBusy &&
              controller.messages.any(
                (m) => m.tools.any(
                  (t) => t.output.contains('Contents from the SSH host.'),
                ),
              ),
          controller,
        );
        await controller.send(
          'Please use write tool to create SSH created.txt.',
        );
        await _until(() => controller.approvals.isNotEmpty, controller);
        expect(ssh.files.containsKey('/home/project/created.txt'), isFalse);
        await controller.replyPermission(controller.approvals.single.id, true);
        await _until(
          () =>
              !controller.isBusy &&
              ssh.files['/home/project/created.txt'] == 'Remote approved edit.',
          controller,
        );
        expect(
          await File('${workspace.path}/created.txt').readAsString(),
          'Approved edit.',
        );
        expect(
          await File('${controller.workspacePath}/created.txt').exists(),
          isFalse,
        );
        expect(
          ssh.calls.every((call) => call['session'] == 'integration-ssh'),
          isTrue,
        );

        await controller.send('Please use command tool on the SSH host.');
        await _until(() => controller.approvals.isNotEmpty, controller);
        expect(ssh.command, isNull);
        await controller.replyPermission(controller.approvals.single.id, true);
        await _until(
          () =>
              !controller.isBusy &&
              controller.messages.any(
                (m) => m.tools.any(
                  (t) => t.output.contains('remote stderr and stdout'),
                ),
              ),
          controller,
        );
        expect(ssh.command, startsWith("cd '/home/project' &&"));
        expect(controller.error, isNull);
      } finally {
        await controller.shutdown();
        controller.dispose();
        registry.dispose();
        await model.close(force: true);
        await directory.delete(recursive: true);
      }
    },
    skip: package == null
        ? 'Set AETHERDESK_ASSISTANT_PACKAGE to a verified portable package to run the real bundled-backend test.'
        : false,
    timeout: const Timeout(Duration(minutes: 3)),
  );
}
