import 'dart:io';
import 'package:ad_remote_desktop/editor/editor_popup_host.dart';
import 'package:ad_remote_desktop/editor/editor_popup_client.dart';
import 'package:ad_remote_desktop/file_workspace/file_open_controller.dart';
import 'package:ad_remote_desktop/workspace.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'file_workspace_review_test.dart' show RemoteFixture;

void main() {
  late Directory cache;
  late RemoteFixture remote;
  late FileOpenController files;
  late EditorPopupOwner owner;
  setUp(() async {
    cache = await Directory.systemTemp.createTemp('aetherdesk-popup-owner-');
    remote = RemoteFixture();
    files = FileOpenController(
      identity: 'user@server:22',
      execute: remote.execute,
      cache: cache,
    );
    owner = EditorPopupOwner(files, refresh: () async {});
  });
  tearDown(() async {
    for (final doc in List.of(files.documents)) {
      await files.close(doc);
    }
    files.dispose();
    await cache.delete(recursive: true);
  });
  test('broadcast snapshots omit only text the popup already has', () async {
    final doc = await files.openRemote('/code.py');
    final first = owner.snapshot(broadcast: true);
    final firstDoc = (first['documents'] as List).first as Map;
    expect(firstDoc['text'], doc.text);
    expect(firstDoc['savedText'], doc.savedText);

    // Nothing changed: a broadcast carries no text the popup already holds.
    final second = owner.snapshot(broadcast: true);
    final secondDoc = (second['documents'] as List).first as Map;
    expect(secondDoc.containsKey('text'), isFalse);
    expect(secondDoc.containsKey('savedText'), isFalse);

    files.changed(doc, 'print("changed")\r\n');
    final third = owner.snapshot(broadcast: true);
    final thirdDoc = (third['documents'] as List).first as Map;
    expect(thirdDoc['text'], 'print("changed")\r\n');

    // A reply invalidates the delta cache: a later broadcast must never rely
    // on text a crossed broadcast may have omitted.
    owner.snapshot();
    final fourth = owner.snapshot(broadcast: true);
    final fourthDoc = (fourth['documents'] as List).first as Map;
    expect(fourthDoc['text'], 'print("changed")\r\n');

    // A closed and reopened document receives its full text again.
    await files.close(doc);
    owner.snapshot(broadcast: true);
    final reopened = await files.openRemote('/code.py');
    final fifth = owner.snapshot(broadcast: true);
    final fifthDoc = (fifth['documents'] as List).first as Map;
    expect(fifthDoc['id'], reopened.id);
    expect(fifthDoc['text'], reopened.text);
  });
  test(
    'popup changes preserve their generations and save through the sole owner',
    () async {
      final opened = await owner.dispatch('openRemote', {
        'path': '/code.py',
        'forceEditor': true,
      });
      final id = opened['resultId'] as String;
      final changed = await owner.dispatch('change', {
        'id': id,
        'text': 'print("hello")\r\n',
        'generation': 5,
      });
      expect(changed['operationError'], isNull);
      expect(files.selected!.generation, 5);
      expect(files.selected!.dirty, true);
      final saved = await owner.dispatch('save', {'id': id});
      expect(saved['operationError'], isNull);
      expect(remote.hostText, 'print("hello")\r\n');
      expect(files.selected!.dirty, false);
      expect(saved['revision'] as int, greaterThan(changed['revision'] as int));
    },
  );
  test(
    'stale and malformed popup changes cannot replace the owner draft',
    () async {
      final doc = await files.openRemote('/code.py');
      await owner.dispatch('change', {
        'id': doc.id,
        'text': 'valuable',
        'generation': 4,
      });
      for (final change in [
        {'id': doc.id, 'text': 'older', 'generation': 3},
        {'id': doc.id, 'text': 'different', 'generation': 4},
        {'id': doc.id, 'text': 'bad\u0000text', 'generation': 5},
        {'id': 'another-workspace', 'text': 'stranger', 'generation': 5},
      ]) {
        expect(
          (await owner.dispatch('change', change))['operationError'],
          isNotNull,
        );
        expect(doc.text, 'valuable');
        expect(doc.generation, 4);
      }
      await owner.dispatch('flushDrafts', {});
      expect(await doc.localFile.readAsString(), 'valuable');
    },
  );
  test(
    'conflicts return preserved document state for the popup dialog',
    () async {
      final doc = await files.openRemote('/code.py');
      await owner.dispatch('change', {
        'id': doc.id,
        'text': 'local draft',
        'generation': 2,
      });
      remote.hostText = 'someone else changed the host';
      final reply = await owner.dispatch('save', {'id': doc.id});
      expect(reply['operationError'], contains('CONFLICT'));
      expect((reply['documents'] as List).first['status'], 'Conflict');
      expect(doc.text, 'local draft');
      expect(await doc.localFile.readAsString(), 'local draft');
    },
  );
  test(
    'editor comparisons cannot write arbitrary files or mutate host paths',
    () async {
      final doc = await files.openRemote('/code.py');
      final refused = await owner.dispatch('execute', {
        'request': {
          'operation': 'download',
          'path': '/code.py',
          'localPath': '${cache.path}/unsafe.txt',
        },
      });
      expect(refused['operationError'], isNotNull);
      expect(await File('${cache.path}/unsafe.txt').exists(), false);
      final blocked = await owner.dispatch('execute', {
        'request': {'operation': 'delete', 'path': '/code.py'},
      });
      expect(blocked['operationError'], isNotNull);
      final allowed = await owner.dispatch('execute', {
        'request': {
          'operation': 'download',
          'path': '/code.py',
          'localPath': '${doc.localFile.parent.parent.path}/compare.tmp',
        },
      });
      expect(allowed['operationError'], isNull);
      expect(allowed['result'], isA<Map>());
    },
  );

  test('typing replies omit only text the popup already has', () async {
    final doc = await files.openRemote('/code.py');
    final initial = owner.snapshot();
    final savedRevision = (initial['documents'] as List).first['savedRevision'];
    final changed = await owner.dispatch('change', {
      'id': doc.id,
      'text': 'new text',
      'generation': 5,
      'knownDocuments': {
        doc.id: {'generation': 5, 'savedRevision': savedRevision},
      },
    });
    final state = (changed['documents'] as List).first as Map;
    expect(state.containsKey('text'), false);
    expect(state.containsKey('savedText'), false);
    expect(state['dirty'], true);
    final rejected = await owner.dispatch('change', {
      'id': doc.id,
      'text': 'wrong text',
      'generation': 5,
      'knownDocuments': {
        doc.id: {'generation': 5, 'savedRevision': savedRevision},
      },
    });
    expect(rejected['operationError'], isNotNull);
    expect((rejected['documents'] as List).first['text'], 'new text');
    await owner.dispatch('save', {'id': doc.id});
    final flush = await owner.dispatch('flushDrafts', {
      'knownDocuments': {
        doc.id: {'generation': 5, 'savedRevision': savedRevision},
      },
    });
    expect((flush['documents'] as List).first['savedText'], 'new text');
    expect(
      (flush['documents'] as List).first['savedRevision'],
      isNot(savedRevision),
    );
  });
  test(
    'real owner and mirror hand over, save and reload generation jumps',
    () async {
      await files.openRemote('/code.py');
      final mirror = PopupFileController(
        identity: files.identity,
        changeDelay: const Duration(hours: 1),
        request: (method, args) =>
            owner.dispatch(method, Map<String, dynamic>.from(args)),
      );
      try {
        mirror.applySnapshot(owner.snapshot());
        final doc = mirror.selected!;
        mirror.changed(doc, 'typed in separate window');
        doc.generation = 10; // Monaco debounce may combine several keystrokes.
        await mirror.flushAll();
        expect(files.selected!.generation, 10);
        expect(files.selected!.text, doc.text);
        await mirror.save(doc);
        expect(remote.hostText, 'typed in separate window');
        expect(doc.dirty, false);
        remote.hostText = 'new host contents';
        await mirror.reload(doc);
        expect(doc.text, 'new host contents');
        expect(doc.generation, 11);
        expect(doc.dirty, false);
      } finally {
        mirror.dispose();
      }
    },
  );
  testWidgets('SFTP-only workspace gives the file explorer the full space', (
    tester,
  ) async {
    const key = ValueKey('files');
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(
          body: WorkspaceSplit(
            explorerVisible: true,
            explorerOnly: true,
            explorer: ColoredBox(key: key, color: Colors.red),
            terminal: Text('terminal'),
          ),
        ),
      ),
    );
    expect(find.text('terminal'), findsNothing);
    expect(
      tester.getSize(find.byKey(key)),
      tester.getSize(find.byType(Scaffold)),
    );
    expect(tester.takeException(), isNull);
  });
}
