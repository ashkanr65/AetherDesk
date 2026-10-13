import 'dart:async';
import 'dart:io';
import 'package:ad_remote_desktop/editor/editor_popup_client.dart';
import 'package:ad_remote_desktop/editor/editor_popup_protocol.dart';
import 'package:ad_remote_desktop/file_workspace/document_session.dart';
import 'package:ad_remote_desktop/file_workspace/file_action_policy.dart';
import 'package:ad_remote_desktop/file_workspace/file_open_controller.dart';
import 'package:flutter_test/flutter_test.dart';

Map<String, dynamic> document({
  String id = 'one',
  String text = 'original',
  String saved = 'original',
  int generation = 0,
  String? path,
  String status = 'Saved to host',
}) => {
  'id': id,
  'remotePath': '/$id.txt',
  'localPath': path ?? '/unused/$id.txt',
  'remote': true,
  'metadata': {'kind': 'file', 'revision': 'revision'},
  'content': {'encoding': 'utf8', 'bom': true},
  'text': text,
  'savedText': saved,
  'generation': generation,
  'dirty': text != saved,
  'saving': false,
  'editorVisible': true,
  'external': false,
  'externalWatching': false,
  'localDigest': 'digest',
  'externalObservedDigest': null,
  'status': status,
  'error': null,
};

Map<String, dynamic> snapshot(Map<String, dynamic> doc, int revision) => {
  'revision': revision,
  'documents': [doc],
  'selectedId': doc['id'],
};

void main() {
  test(
    'initial snapshot opens text at generation zero and roundtrips encoding',
    () async {
      final owner = FileOpenController(
        identity: 'host',
        execute: (request, {job, onProgress}) async => {},
      );
      final doc =
          DocumentSession(
              id: 'one',
              remotePath: '/one.txt',
              localFile: File('/unused/one.txt'),
              metadata: {'kind': 'file'},
              content: TextFileContent('hello', 'utf16le', true),
            )
            ..text = 'hello'
            ..savedText = 'hello'
            ..editorVisible = true;
      owner.documents.add(doc);
      owner.selectedId = doc.id;
      final mirror = PopupFileController(
        identity: 'host',
        request: (method, args) async => {},
      );
      mirror.applySnapshot(editorPopupSnapshot(owner, revision: 1));
      expect(mirror.selected!.text, 'hello');
      expect(mirror.selected!.content!.encoding, 'utf16le');
      expect(mirror.selected!.content!.bom, isTrue);
      // Avoid the ordinary controller's disposal journal because this fixture is read-only.
      owner.documents.clear();
      owner.dispose();
      mirror.dispose();
    },
  );

  test(
    'edits coalesce and flush to the owner without writing its working copy',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-popup-test-',
      );
      addTearDown(() => directory.delete(recursive: true));
      final copy = File('${directory.path}/one.txt');
      await copy.writeAsString('original');
      var current = document(path: copy.path);
      var revision = 1;
      final calls = <String>[];
      final changes = <Map<String, Object?>>[];
      final mirror = PopupFileController(
        identity: 'host',
        changeDelay: const Duration(hours: 1),
        request: (method, args) async {
          calls.add(method);
          if (method == 'change') {
            changes.add(Map.of(args));
            current = document(
              path: copy.path,
              text: args['text'] as String,
              generation: args['generation'] as int,
            );
          }
          return snapshot(current, ++revision);
        },
      );
      mirror.applySnapshot(snapshot(current, revision));
      final doc = mirror.selected!;
      mirror.changed(doc, 'one');
      mirror.changed(doc, 'two');
      mirror.changed(doc, 'three');
      await mirror.flushAll();
      expect(changes, hasLength(1));
      expect(changes.single['text'], 'three');
      expect(changes.single['generation'], 3);
      expect(calls, ['change', 'flushDrafts']);
      expect(await copy.readAsString(), 'original');
      expect(await File('${directory.path}/document.json').exists(), isFalse);
      mirror.dispose();
    },
  );

  test(
    'a delayed acknowledgement never replaces typing made in flight',
    () async {
      var current = document();
      var revision = 1;
      final first = Completer<Map<String, dynamic>>();
      final started = Completer<void>();
      final sent = <String>[];
      final mirror = PopupFileController(
        identity: 'host',
        changeDelay: const Duration(hours: 1),
        request: (method, args) async {
          if (method == 'change') {
            final text = args['text'] as String;
            sent.add(text);
            current = document(
              text: text,
              generation: args['generation'] as int,
            );
            if (sent.length == 1) {
              started.complete();
              return first.future;
            }
          }
          return snapshot(current, ++revision);
        },
      );
      mirror.applySnapshot(snapshot(current, revision));
      final doc = mirror.selected!;
      mirror.changed(doc, 'first');
      final flushing = mirror.flushAll();
      await started.future;
      mirror.changed(doc, 'latest');
      mirror.applySnapshot(
        snapshot(document(text: 'first', generation: 1), ++revision),
      );
      expect(doc.text, 'latest');
      expect(doc.generation, 2);
      first.complete(
        snapshot(document(text: 'first', generation: 1), ++revision),
      );
      await flushing;
      expect(sent, ['first', 'latest']);
      expect(doc.text, 'latest');
      mirror.dispose();
    },
  );

  test(
    'Monaco generation jump reaches owner so a later reload can replace text',
    () async {
      var current = document();
      var revision = 1;
      final generations = <int>[];
      final mirror = PopupFileController(
        identity: 'host',
        changeDelay: const Duration(hours: 1),
        request: (method, args) async {
          if (method == 'change') {
            final generation = args['generation'] as int;
            generations.add(generation);
            current = document(
              text: args['text'] as String,
              generation: generation,
            );
          } else if (method == 'reload') {
            current = document(
              text: 'new host text',
              saved: 'new host text',
              generation: (current['generation'] as int) + 1,
            );
          }
          return snapshot(current, ++revision);
        },
      );
      mirror.applySnapshot(snapshot(current, revision));
      final doc = mirror.selected!;
      mirror.changed(doc, 'debounced typing');
      // This is the order used by EditorWorkspace._receive for a Monaco model.
      doc.generation = 5;
      await mirror.reload(doc);
      expect(generations, [5]);
      expect(doc.generation, 6);
      expect(doc.text, 'new host text');
      expect(doc.dirty, isFalse);
      mirror.dispose();
    },
  );

  test(
    'unacknowledged typing survives an owner revision ahead of the popup',
    () async {
      final mirror = PopupFileController(
        identity: 'host',
        request: (method, args) async => {},
        changeDelay: const Duration(hours: 1),
      );
      mirror.applySnapshot(snapshot(document(), 1));
      final doc = mirror.selected!;
      mirror.changed(doc, 'my unsent text');
      mirror.applySnapshot(
        snapshot(
          document(
            text: 'host replacement',
            saved: 'host replacement',
            generation: 5,
          ),
          2,
        ),
      );
      expect(doc.text, 'my unsent text');
      expect(doc.dirty, isTrue);
      expect(doc.error, contains('different document revision'));
      mirror.dispose();
    },
  );

  test(
    'older snapshot revision cannot undo a saved status or selection',
    () async {
      final mirror = PopupFileController(
        identity: 'host',
        request: (method, args) async => {},
      );
      mirror.applySnapshot(
        snapshot(document(text: 'saved', saved: 'saved', generation: 1), 8),
      );
      mirror.applySnapshot(
        snapshot(document(text: 'old', generation: 0, status: 'Modified'), 7),
      );
      expect(mirror.selected!.text, 'saved');
      expect(mirror.selected!.status, 'Saved to host');
      expect(mirror.selected!.dirty, isFalse);
      mirror.dispose();
    },
  );

  test(
    'owner save conflict state is applied before the error reaches the dialog',
    () async {
      var current = document();
      var revision = 1;
      final mirror = PopupFileController(
        identity: 'host',
        changeDelay: const Duration(hours: 1),
        request: (method, args) async {
          if (method == 'change')
            current = document(
              text: args['text'] as String,
              generation: args['generation'] as int,
            );
          final reply = snapshot(current, ++revision);
          if (method == 'save') {
            current['status'] = 'Conflict';
            current['error'] = 'CONFLICT: host changed';
            reply['operationError'] = 'CONFLICT: host changed';
          }
          return reply;
        },
      );
      mirror.applySnapshot(snapshot(current, revision));
      final doc = mirror.selected!;
      mirror.changed(doc, 'draft');
      await expectLater(
        mirror.save(doc),
        throwsA(
          predicate((Object error) => error.toString().contains('CONFLICT')),
        ),
      );
      expect(doc.text, 'draft');
      expect(doc.dirty, isTrue);
      expect(doc.status, 'Conflict');
      mirror.dispose();
    },
  );

  test(
    'failed handover retains the text and retries instead of writing a local journal',
    () async {
      var current = document();
      var revision = 1;
      var fail = true;
      final mirror = PopupFileController(
        identity: 'host',
        changeDelay: const Duration(hours: 1),
        request: (method, args) async {
          if (method == 'change') {
            if (fail)
              return {
                ...snapshot(current, ++revision),
                'operationError': 'temporarily unavailable',
              };
            current = document(
              text: args['text'] as String,
              generation: args['generation'] as int,
            );
          }
          return snapshot(current, ++revision);
        },
      );
      mirror.applySnapshot(snapshot(current, revision));
      final doc = mirror.selected!;
      mirror.changed(doc, 'keep me');
      await expectLater(mirror.flushAll(), throwsStateError);
      expect(doc.text, 'keep me');
      expect(doc.error, contains('Could not preserve changes'));
      fail = false;
      await mirror.flushAll();
      expect(doc.text, 'keep me');
      expect(doc.error, isNull);
      mirror.dispose();
    },
  );

  test(
    'metadata delta retains current text and dirty state through typing',
    () async {
      final mirror = PopupFileController(
        identity: 'host',
        changeDelay: const Duration(hours: 1),
        request: (method, args) async =>
            snapshot(document(text: 'draft', generation: 2), 5),
      );
      try {
        final initial = document()..['savedRevision'] = 1;
        mirror.applySnapshot(snapshot(initial, 1));
        final doc = mirror.selected!;
        mirror.changed(doc, 'd');
        final delta = document(text: 'd', generation: 1)..['savedRevision'] = 1;
        delta.remove('text');
        delta.remove('savedText');
        mirror.changed(doc, 'draft');
        mirror.applySnapshot(snapshot(delta, 2));
        expect(doc.text, 'draft');
        expect(doc.generation, 2);
        expect(doc.savedText, 'original');
        expect(doc.dirty, true);
        final unknownSaved = {...delta, 'savedRevision': 2};
        expect(
          () => mirror.applySnapshot(snapshot(unknownSaved, 3)),
          throwsFormatException,
        );
        final missing = document(id: 'new')..remove('text');
        expect(
          () => mirror.applySnapshot(snapshot(missing, 4)),
          throwsFormatException,
        );
      } finally {
        mirror.dispose();
      }
    },
  );
  test(
    'host file requests allow slow transfers without shortening draft calls',
    () {
      expect(editorPopupRequestTimeout('save'), const Duration(minutes: 2));
      expect(
        editorPopupRequestTimeout('openRemote'),
        const Duration(minutes: 2),
      );
      expect(
        editorPopupRequestTimeout('flushDrafts'),
        const Duration(seconds: 30),
      );
    },
  );
}
