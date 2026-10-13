import 'dart:io';

import 'package:ad_remote_desktop/assistant/workspace_tools.dart';
import 'package:ad_remote_desktop/tools/tool_registry.dart';
import 'package:ad_remote_desktop/tools/tools_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'tool_registry_test.dart' show manifest;

class FakeRunner extends WorkspaceTools {
  FakeRunner(
    Directory directory,
    List<AssistantRemoteHost> Function() hosts,
    ToolRegistry registry,
  ) : super(localDirectory: directory, remoteHosts: hosts, registry: registry);
  int executions = 0;
  Map<String, String>? arguments;
  String? revision;
  @override
  Future<Object> executeTool(
    String id, {
    required String expectedRevision,
    Map<String, String> arguments = const {},
  }) async {
    executions++;
    revision = expectedRevision;
    this.arguments = arguments;
    return {'stdout': 'tool output', 'exitCode': 0};
  }
}

Future<void> tapLive(WidgetTester tester, Finder finder) async {
  await tester.runAsync(() => tester.tap(finder));
}

void main() {
  late Directory directory;
  late ToolRegistry registry;
  setUp(() async {
    directory = await Directory.systemTemp.createTemp(
      'aetherdesk-tools-panel-',
    );
    registry = ToolRegistry(directory: directory);
  });
  tearDown(() async {
    registry.dispose();
    await directory.delete(recursive: true);
  });

  Future<void> showPanel(
    WidgetTester tester, {
    ToolRunnerFactory? factory,
    List<AssistantRemoteHost> hosts = const [],
  }) async {
    await tester.runAsync(
      () => tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 258,
              child: ToolsPanel(
                registry: registry,
                remoteHosts: () => hosts,
                runnerFactory: factory,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(registry.reload);
    await tester.pumpAndSettle();
  }

  testWidgets(
    'empty panel explains creation and installing updates it without AI',
    (tester) async {
      await showPanel(tester);
      expect(find.textContaining('Ask AI Assistant'), findsOneWidget);
      await tester.runAsync(
        () => registry.install(manifest(), expectedRevision: 'new'),
      );
      await tester.pumpAndSettle();
      expect(find.text('test-tool'), findsOneWidget);
      expect(find.text('Run'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tapLive(tester, find.text('Inspect'));
      await tester.pumpAndSettle();
      expect(find.textContaining('Write-Output test'), findsOneWidget);
    },
  );

  testWidgets('run waits for exact script review and keeps parameter values', (
    tester,
  ) async {
    final tool = (await tester.runAsync(
      () => registry.install(manifest(), expectedRevision: 'new'),
    ))!;
    FakeRunner? runner;
    await showPanel(
      tester,
      factory: (directory, hosts) =>
          runner = FakeRunner(directory, hosts, registry),
    );
    await tapLive(tester, find.text('Run'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.widgetWithText(TextField, 'Working folder'),
      directory.path,
    );
    await tester.enterText(find.widgetWithText(TextField, 'Message'), "a';b");
    await tapLive(tester, find.text('Review script'));
    await tester.pumpAndSettle();
    expect(runner, isNull);
    expect(find.textContaining("a'';b"), findsOneWidget);
    await tapLive(tester, find.text('Run tool'));
    await tester.runAsync(() async {
      for (var i = 0; i < 30 && runner?.executions != 1; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
    });
    await tester.pumpAndSettle();
    expect(runner!.executions, 1);
    expect(runner!.revision, tool.revision);
    expect(runner!.arguments, {'message': "a';b"});
    expect(find.textContaining('tool output'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'SSH tool requires connection and removal requires confirmation',
    (tester) async {
      await tester.runAsync(
        () => registry.install(
          manifest(target: 'ssh', script: 'pwd'),
          expectedRevision: 'new',
        ),
      );
      await showPanel(tester);
      await tapLive(tester, find.text('Run'));
      await tester.pumpAndSettle();
      await tapLive(tester, find.text('Review script'));
      await tester.pumpAndSettle();
      expect(find.textContaining('Connect an SSH host'), findsOneWidget);
      await tapLive(tester, find.text('Cancel'));
      await tester.pumpAndSettle();
      await tapLive(tester, find.byTooltip('Remove test-tool'));
      await tester.pumpAndSettle();
      expect(registry.tools, hasLength(1));
      await tapLive(tester, find.text('Remove'));
      await tester.pump();
      await tester.runAsync(registry.reload);
      await tester.pumpAndSettle();
      expect(registry.tools, isEmpty);
    },
  );
}
