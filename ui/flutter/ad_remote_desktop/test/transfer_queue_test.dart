import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:ad_remote_desktop/file_workspace/remote_file_repository.dart';
import 'package:ad_remote_desktop/file_workspace/transfer_queue.dart';

class FakeHost {
  final files = <String, String>{};
  final directories = <String>{'/host'};
  final links = <String>{};
  final canonicalPaths = <String, String>{};
  final calls = <Map<String, Object?>>[];
  List<String>? listing;
  bool interruptDownload = false;
  bool cancelDownload = false;

  String revision(String path) => 'revision:${files[path]}';
  Future<Map<String, dynamic>> execute(
    Map<String, Object?> request, {
    FileJob? job,
    FileProgressCallback? onProgress,
  }) async {
    calls.add(Map.of(request));
    final path = request['path']! as String;
    final operation = request['operation'];
    if (operation == 'stat' || operation == 'revision') {
      if (!files.containsKey(path) &&
          !directories.contains(path) &&
          !links.contains(path)) {
        throw StateError('file no longer exists');
      }
      return {
        'metadata': {
          'kind': links.contains(path)
              ? 'symlink'
              : directories.contains(path)
              ? 'directory'
              : 'file',
          'size': files[path]?.length ?? 0,
          'canonicalPath': canonicalPaths[path] ?? path,
          'revision': revision(path),
        },
      };
    }
    if (operation == 'list') {
      final prefix = '$path/';
      final names =
          listing ??
          {...directories, ...files.keys, ...links}
              .where(
                (item) =>
                    item.startsWith(prefix) &&
                    !item.substring(prefix.length).contains('/'),
              )
              .map((item) => item.substring(prefix.length))
              .toList();
      return {
        'entries': [
          for (final name in names) {'name': name},
        ],
      };
    }
    if (operation == 'mkdir') {
      directories.add(path);
      return {};
    }
    if (operation == 'save') {
      if (request['expectedRevision'] != revision(path))
        throw StateError('CONFLICT');
    }
    if (operation == 'save' || operation == 'upload') {
      if (operation == 'upload' && files.containsKey(path))
        throw StateError('Already exists');
      files[path] = await File(request['localPath']! as String).readAsString();
      onProgress?.call(files[path]!.length, files[path]!.length);
      return {};
    }
    if (operation == 'download') {
      await File(request['localPath']! as String).writeAsString(files[path]!);
      if (interruptDownload) throw StateError('Connection lost');
      if (cancelDownload) job!.cancelled = true;
      onProgress?.call(files[path]!.length, files[path]!.length);
      return {};
    }
    throw StateError('Unexpected request: $request');
  }
}

void main() {
  late Directory local;
  late FakeHost host;
  late WorkspaceTransfer transfer;
  setUp(() async {
    local = await Directory.systemTemp.createTemp('aetherdesk-transfer-test-');
    host = FakeHost();
    transfer = WorkspaceTransfer(execute: host.execute);
  });
  tearDown(() async {
    await local.delete(recursive: true);
  });

  test(
    'recursive upload creates empty folders and reports aggregate progress',
    () async {
      await Directory('${local.path}/empty').create();
      await Directory('${local.path}/nested').create();
      await File('${local.path}/nested/a.txt').writeAsString('abc');
      await File('${local.path}/b.txt').writeAsString('hello');
      var progress = <int>[];
      await transfer.upload(
        local.path,
        '/host/tree',
        onProgress: (done, total) {
          progress = [done, total];
        },
      );
      expect(host.directories, contains('/host/tree/empty'));
      expect(host.files['/host/tree/nested/a.txt'], 'abc');
      expect(host.files['/host/tree/b.txt'], 'hello');
      expect(progress, [8, 8]);
    },
  );

  test('upload refuses existing destination without confirmation', () async {
    final file = await File('${local.path}/a').writeAsString('new');
    host.files['/host/a'] = 'original';
    await expectLater(transfer.upload(file.path, '/host/a'), throwsStateError);
    expect(host.files['/host/a'], 'original');
    expect(host.calls.where((call) => call['operation'] == 'save'), isEmpty);
  });

  test('confirmed upload saves only the approved remote revision', () async {
    final file = await File('${local.path}/a').writeAsString('new');
    host.files['/host/a'] = 'original';
    await expectLater(
      transfer.upload(
        file.path,
        '/host/a',
        confirmReplace: (_) async {
          host.files['/host/a'] = 'someone else edited';
          return true;
        },
      ),
      throwsStateError,
    );
    expect(host.files['/host/a'], 'someone else edited');
    final save = host.calls.last;
    expect(save['expectedRevision'], 'revision:original');
    expect(save['overwrite'], isFalse);
  });

  test(
    'recursive download creates nested files and empty directories',
    () async {
      host.directories.addAll([
        '/host/tree',
        '/host/tree/empty',
        '/host/tree/nested',
      ]);
      host.files['/host/tree/nested/a.txt'] = 'hello';
      await transfer.download('/host/tree', '${local.path}/copy');
      expect(await Directory('${local.path}/copy/empty').exists(), isTrue);
      expect(
        await File('${local.path}/copy/nested/a.txt').readAsString(),
        'hello',
      );
    },
  );

  test(
    'failed download preserves destination and removes its private stage',
    () async {
      final file = await File('${local.path}/a.txt').writeAsString('original');
      host.files['/host/a.txt'] = 'incomplete';
      host.interruptDownload = true;
      await expectLater(
        transfer.download(
          '/host/a.txt',
          file.path,
          confirmReplace: (_) async => true,
        ),
        throwsStateError,
      );
      expect(await file.readAsString(), 'original');
      expect(await local.list().length, 1);
    },
  );

  test('cancelled download does not commit its staged data', () async {
    final file = await File('${local.path}/a.txt').writeAsString('original');
    host.files['/host/a.txt'] = 'new';
    host.cancelDownload = true;
    await expectLater(
      transfer.download(
        '/host/a.txt',
        file.path,
        job: FileJob(),
        confirmReplace: (_) async => true,
      ),
      throwsStateError,
    );
    expect(await file.readAsString(), 'original');
  });

  test('local changes during confirmation are protected', () async {
    final file = await File('${local.path}/a.txt').writeAsString('original');
    host.files['/host/a.txt'] = 'host';
    await expectLater(
      transfer.download(
        '/host/a.txt',
        file.path,
        confirmReplace: (_) async {
          await file.writeAsString('new local edit');
          return true;
        },
      ),
      throwsStateError,
    );
    expect(await file.readAsString(), 'new local edit');
  });

  test(
    'host Windows case collisions fail before local files are written',
    () async {
      host.files.addAll({'/host/A.txt': 'one', '/host/a.txt': 'two'});
      await expectLater(
        transfer.download('/host', '${local.path}/copy'),
        throwsStateError,
      );
      expect(await Directory('${local.path}/copy').exists(), isFalse);
      expect(
        host.calls.where((call) => call['operation'] == 'download'),
        isEmpty,
      );
    },
  );

  test(
    'unsafe Windows filenames and traversal fail before local writes',
    () async {
      for (final name in [
        '../escape',
        r'..\escape',
        'NUL.txt',
        'COM1',
        'trailing.',
        'a:b',
        'bad?name',
      ]) {
        host.listing = [name];
        await expectLater(
          transfer.download('/host', '${local.path}/copy'),
          throwsFormatException,
        );
        expect(await Directory('${local.path}/copy').exists(), isFalse);
      }
    },
  );

  test('remote symbolic links are not followed', () async {
    host.links.add('/host/shortcut');
    await expectLater(
      transfer.download('/host', '${local.path}/copy'),
      throwsStateError,
    );
    expect(
      host.calls.where((call) => call['operation'] == 'download'),
      isEmpty,
    );
  });

  test('pre-cancelled operation does not contact the host', () async {
    final job = FileJob()..cancelled = true;
    await expectLater(
      transfer.download('/host', '${local.path}/copy', job: job),
      throwsStateError,
    );
    expect(host.calls, isEmpty);
  });
}
