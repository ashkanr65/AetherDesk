import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:ad_remote_desktop/file_workspace/file_open_controller.dart';
import 'package:ad_remote_desktop/file_workspace/local_app_launcher.dart';
import 'package:ad_remote_desktop/file_workspace/remote_file_repository.dart';
import 'file_workspace_review_test.dart' show RemoteFixture;

class NoLaunch extends LocalAppLauncher {
  const NoLaunch();
  @override
  Future<void> open(String path, {String? application}) async {}
}

void main() {
  test(
    'failed download can be retried and failed reload preserves the pending draft',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-reload-test-',
      );
      final remote = RemoteFixture();
      var fail = true;
      Future<Map<String, dynamic>> execute(
        Map<String, Object?> request, {
        FileJob? job,
        FileProgressCallback? onProgress,
      }) async {
        if (request['operation'] == 'download' && fail) {
          await File(request['localPath'] as String).writeAsString('partial');
          throw StateError('Connection lost');
        }
        return remote.execute(request, job: job, onProgress: onProgress);
      }

      final controller = FileOpenController(
        identity: 'test',
        execute: execute,
        cache: directory,
      );
      try {
        await expectLater(controller.openRemote('/file.txt'), throwsStateError);
        fail = false;
        final doc = await controller.openRemote('/file.txt');
        controller.changed(doc, 'valuable draft');
        await controller.flush(doc);
        fail = true;
        await expectLater(controller.reload(doc), throwsStateError);
        expect(doc.text, 'valuable draft');
        expect(await doc.localFile.readAsString(), 'valuable draft');
        expect(doc.dirty, true);
        await controller.close(doc);
      } finally {
        controller.dispose();
        await directory.delete(recursive: true);
      }
    },
  );
  test(
    'recovered external draft waits for an actual application save',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'aetherdesk-external-test-',
      );
      final remote = RemoteFixture()..hostText = '\u0000original';
      FileOpenController create() => FileOpenController(
        identity: 'test',
        execute: remote.execute,
        cache: directory,
        launcher: const NoLaunch(),
      );
      var controller = create();
      try {
        var doc = await controller.openRemote('/layout.gds');
        await doc.localFile.writeAsString('\u0000recovered');
        await controller.close(doc);
        controller.dispose();
        controller = create();
        doc = await controller.openRemote('/layout.gds');
        expect(doc.dirty, true);
        await controller.openApplication(doc);
        await controller.checkExternalChanges();
        expect(remote.hostText, '\u0000original');
        await doc.localFile.writeAsString('\u0000new save');
        await controller.checkExternalChanges();
        expect(remote.hostText, '\u0000new save');
        expect(doc.dirty, false);
        await controller.close(doc);
      } finally {
        controller.dispose();
        await directory.delete(recursive: true);
      }
    },
  );
}
