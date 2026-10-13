import 'dart:convert';
import 'dart:io';

import 'package:ad_remote_desktop/tools/tool_registry.dart';
import 'package:flutter_test/flutter_test.dart';

Map<String, dynamic> manifest({
  String id = 'test-tool',
  String target = 'windows',
  String script = 'Write-Output test',
}) => {
  'schemaVersion': 1,
  'id': id,
  'title': id,
  'description': 'Test reusable tool',
  'version': '1.0.0',
  'target': target,
  'script': script,
  'parameters': [
    {'name': 'message', 'label': 'Message', 'required': true},
  ],
};

void main() {
  late Directory directory;
  late ToolRegistry registry;
  setUp(() async {
    directory = await Directory.systemTemp.createTemp('aetherdesk-tools-test-');
    registry = ToolRegistry(directory: directory);
  });
  tearDown(() async {
    registry.dispose();
    await directory.delete(recursive: true);
  });

  test(
    'manifest rejects traversal, device names, unsupported fields and invalid parameters',
    () {
      for (final id in [
        '../escape',
        'a/b',
        'con',
        'lpt1',
        'test..tool',
        'Tool',
      ]) {
        expect(
          () => UserTool.fromJson(manifest(id: id)),
          throwsFormatException,
        );
      }
      for (final patch in [
        {'schemaVersion': 2},
        {'target': 'anything'},
        {'version': 'latest'},
        {'autoRun': true},
        {'script': 'x\u0000'},
        {
          'parameters': [
            {'name': 'x;pwd', 'label': 'Value'},
          ],
        },
        {
          'parameters': [
            {'name': 'x', 'label': 'Value'},
            {'name': 'X', 'label': 'Other'},
          ],
        },
      ]) {
        expect(
          () => UserTool.fromJson({...manifest(), ...patch}),
          throwsFormatException,
        );
      }
    },
  );

  test('parameter data is shell-quoted without replacing script content', () {
    const value = "quote'; Write-Output injected; \$(boom)\nnext line";
    final windows = UserTool.fromJson(
      manifest(script: r'Write-Output $env:AETHER_TOOL_message'),
    );
    expect(
      windows.command({'message': value}),
      "\$env:AETHER_TOOL_message = 'quote''; Write-Output injected; \$(boom)\nnext line';\nWrite-Output \$env:AETHER_TOOL_message",
    );
    final ssh = UserTool.fromJson(
      manifest(target: 'ssh', script: r'printf "%s" "$AETHER_TOOL_message"'),
    );
    expect(
      ssh.command({'message': value}),
      startsWith(
        "AETHER_TOOL_message='quote'\\''; Write-Output injected; \$(boom)\nnext line'; export AETHER_TOOL_message;\n",
      ),
    );
    expect(() => windows.command({}), throwsFormatException);
    expect(() => windows.command({'other': 'value'}), throwsFormatException);
  });

  test(
    'install persists only and protects revision on update and remove',
    () async {
      var notifications = 0;
      registry.addListener(() => notifications++);
      final created = await registry.install(
        manifest(script: "Set-Content should-not-exist.txt bad"),
        expectedRevision: 'new',
      );
      expect(
        await File('${directory.path}/should-not-exist.txt').exists(),
        false,
      );
      expect(created.revision, hasLength(64));
      expect(registry.tools.single.id, 'test-tool');
      await expectLater(
        registry.install(manifest(), expectedRevision: 'new'),
        throwsStateError,
      );
      await File(
        '${directory.path}/test-tool.json',
      ).writeAsString(jsonEncode(manifest(script: 'changed externally')));
      await expectLater(
        registry.install(manifest(), expectedRevision: created.revision),
        throwsStateError,
      );
      await expectLater(
        registry.remove(created.id, expectedRevision: created.revision),
        throwsStateError,
      );
      final current = await registry.read(created.id);
      final updated = await registry.install(
        manifest(script: 'updated'),
        expectedRevision: current.revision,
      );
      await registry.remove(updated.id, expectedRevision: updated.revision);
      expect(registry.tools, isEmpty);
      expect(notifications, 3);
    },
  );

  test(
    'simultaneous creates cannot overwrite and invalid manifests are reported',
    () async {
      final outcomes = await Future.wait([
        for (var i = 0; i < 2; i++)
          registry
              .install(manifest(), expectedRevision: 'new')
              .then<Object>((value) => value, onError: (Object error) => error),
      ]);
      expect(outcomes.whereType<UserTool>(), hasLength(1));
      expect(outcomes.whereType<StateError>(), hasLength(1));
      await File('${directory.path}/broken.json').writeAsString('{');
      await File(
        '${directory.path}/wrong-id.json',
      ).writeAsString(jsonEncode(manifest()));
      await File(
        '${directory.path}/oversized.json',
      ).writeAsString('x' * (ToolRegistry.maxManifestBytes + 1));
      await registry.reload();
      expect(registry.tools, hasLength(1));
      expect(registry.invalidFiles.toSet(), {
        'broken.json',
        'wrong-id.json',
        'oversized.json',
      });
    },
  );
}
