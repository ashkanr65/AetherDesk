import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ad_remote_desktop/file_workspace/file_action_policy.dart';
import 'package:ad_remote_desktop/file_workspace/file_associations.dart';
import 'package:ad_remote_desktop/file_workspace/file_open_controller.dart';
import 'package:ad_remote_desktop/file_workspace/remote_file_repository.dart';

class RemoteFixture {
  String hostText = 'original';
  String key = 'SHA256:verified-key';
  int downloads = 0;
  Completer<void>? saveStarted;
  Completer<void>? releaseSave;
  String get revision => sha256.convert(utf8.encode(hostText)).toString();
  Map<String, dynamic> metadata(String path) => {
    'kind': 'file',
    'canonicalPath': path,
    'path': path,
    'revision': revision,
    'atomicReplace': true,
  };
  Future<Map<String, dynamic>> execute(
    Map<String, Object?> request, {
    FileJob? job,
    FileProgressCallback? onProgress,
  }) async {
    final path = request['path'] as String;
    if (request['operation'] == 'download') {
      downloads++;
      await File(request['localPath'] as String).writeAsString(hostText);
    } else if (request['operation'] == 'save') {
      final snapshot = File(request['localPath'] as String);
      final originalSnapshot = await snapshot.readAsString();
      saveStarted?.complete();
      if (releaseSave != null) await releaseSave!.future;
      expect(await snapshot.readAsString(), originalSnapshot);
      if (request['overwrite'] != true &&
          request['expectedRevision'] != revision)
        throw StateError('CONFLICT: changed remotely');
      hostText = originalSnapshot;
    }
    return {
      'metadata': metadata(path),
      'hostIdentity': {
        'host': 'server',
        'port': 22,
        'username': 'user',
        'keyFingerprint': key,
      },
    };
  }
}

void main() {
  test(
    'binary controls and malformed UTF16 are rejected; valid Unicode and line endings round trip',
    () {
      for (final bytes in [
        <int>[65, 1, 66],
        <int>[255, 254, 0, 216],
        <int>[254, 255, 220, 0],
        utf8.encode('%PDF-1.7\nbody'),
      ]) {
        expect(TextFileContent.decode(Uint8List.fromList(bytes)), isNull);
      }
      final bytes = Uint8List.fromList([
        255,
        254,
        65,
        0,
        13,
        0,
        10,
        0,
        61,
        216,
        0,
        222,
      ]);
      final decoded = TextFileContent.decode(bytes)!;
      expect(decoded.text, 'A\r\n😀');
      expect(decoded.encode(decoded.text), bytes);
      expect(
        TextFileContent.decode(
          Uint8List.fromList(utf8.encode('const a = "ñ";\n')),
        ),
        isNotNull,
      );
    },
  );

  test(
    'associations accept extensions and paths and preserve concurrent settings',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-association-test-',
      );
      try {
        final first = FileAssociations(directory: directory);
        final second = FileAssociations(directory: directory);
        await Future.wait([
          first.set('gds', 'klayout.exe'),
          second.set('.CPP', 'editor'),
        ]);
        await first.load();
        expect(first.forPath('/host/design.GDS'), 'klayout.exe');
        expect(first.forPath('/host/code.cpp'), 'editor');
        await first.set('/host/notes.txt', 'editor');
        final restored = FileAssociations(directory: directory);
        await restored.load();
        expect(restored.forPath('file.txt'), 'editor');
        expect(restored.forPath('extensionless'), isNull);
      } finally {
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'typing during save preserves new draft while host gets immutable requested snapshot',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-draft-test-',
      );
      final remote = RemoteFixture();
      final controller = FileOpenController(
        identity: 'user@server:22',
        execute: remote.execute,
        cache: Directory('${directory.path}/copies'),
        associations: FileAssociations(
          directory: Directory('${directory.path}/settings'),
        ),
      );
      try {
        await controller.initialize();
        final doc = await controller.openRemote('/file.txt');
        controller.changed(doc, 'first edit');
        remote.saveStarted = Completer<void>();
        remote.releaseSave = Completer<void>();
        final save = controller.save(doc);
        await remote.saveStarted!.future;
        controller.changed(doc, 'second edit');
        await controller.flush(doc);
        remote.releaseSave!.complete();
        await save;
        expect(remote.hostText, 'first edit');
        expect(doc.text, 'second edit');
        expect(doc.savedText, 'first edit');
        expect(doc.dirty, true);
        expect(await doc.localFile.readAsString(), 'second edit');
        await controller.close(doc);
      } finally {
        controller.dispose();
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'local draft never changes original before explicit save and detects outside edits',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-local-draft-test-',
      );
      final original = File('${directory.path}/original.txt');
      await original.writeAsString('original');
      final controller = FileOpenController(
        identity: 'user@server:22',
        execute: RemoteFixture().execute,
        cache: Directory('${directory.path}/copies'),
      );
      try {
        final doc = await controller.openLocal(original.path);
        controller.changed(doc, 'draft');
        await controller.flush(doc);
        expect(await original.readAsString(), 'original');
        await original.writeAsString('outside edit');
        await expectLater(controller.save(doc), throwsStateError);
        expect(await original.readAsString(), 'outside edit');
        expect(doc.dirty, true);
        await controller.close(doc);
      } finally {
        controller.dispose();
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'draft recovery survives reopen and a missing journal never destroys its working copy',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-recovery-test-',
      );
      final remote = RemoteFixture();
      FileOpenController makeController() => FileOpenController(
        identity: 'user@server:22',
        execute: remote.execute,
        cache: Directory('${directory.path}/copies'),
      );
      var controller = makeController();
      try {
        var doc = await controller.openRemote('/file.txt');
        controller.changed(doc, 'recover me');
        await controller.flush(doc);
        await controller.close(doc);
        controller.dispose();
        controller = makeController();
        doc = await controller.openRemote('/file.txt');
        expect(doc.text, 'recover me');
        expect(doc.dirty, true);
        expect(remote.downloads, 1);
        final working = doc.localFile;
        final manifest = File('${working.parent.parent.path}/document.json');
        await controller.close(doc);
        controller.dispose();
        await manifest.delete();
        controller = makeController();
        await expectLater(controller.openRemote('/file.txt'), throwsStateError);
        expect(await working.readAsString(), 'recover me');
        expect(remote.downloads, 1);
      } finally {
        controller.dispose();
        await directory.delete(recursive: true);
      }
    },
  );

  test(
    'verified host keys separate recovery copies for identical host paths',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-host-test-',
      );
      final remote = RemoteFixture();
      final controller = FileOpenController(
        identity: 'user@server:22',
        execute: remote.execute,
        cache: Directory('${directory.path}/copies'),
      );
      try {
        final original = await controller.openRemote('/file.txt');
        controller.changed(original, 'private draft');
        await controller.flush(original);
        await controller.close(original);
        remote.key = 'SHA256:another-verified-key';
        remote.hostText = 'another server';
        final replacement = await controller.openRemote('/file.txt');
        expect(replacement.id, isNot(original.id));
        expect(replacement.text, 'another server');
        expect(await original.localFile.readAsString(), 'private draft');
        await controller.close(replacement);
      } finally {
        controller.dispose();
        await directory.delete(recursive: true);
      }
    },
  );
  test(
    'forcing a binary file into editor preserves a valid cache for later external opening',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-binary-retry-test-',
      );
      final remote = RemoteFixture()
        ..hostText = '%PDF-1.7\nASCII binary container';
      final controller = FileOpenController(
        identity: 'user@server:22',
        execute: remote.execute,
        cache: Directory('${directory.path}/copies'),
      );
      try {
        await expectLater(
          controller.openRemote('/drawing.pdf', forceEditor: true),
          throwsStateError,
        );
        final doc = await controller.openRemote('/drawing.pdf');
        expect(doc.external, true);
        expect(doc.dirty, false);
        await controller.close(doc);
      } finally {
        controller.dispose();
        await directory.delete(recursive: true);
      }
    },
  );
}
