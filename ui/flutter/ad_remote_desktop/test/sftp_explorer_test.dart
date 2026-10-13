import 'dart:async';
import 'dart:io';
import 'package:ad_remote_desktop/workspace.dart';
import 'package:ad_remote_desktop/native_bridge.dart';
import 'package:file_selector_platform_interface/file_selector_platform_interface.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class Files extends FileSelectorPlatform {
  late String uploadPath;
  late String downloadPath;
  @override
  Future<List<XFile>> openFiles({
    List<XTypeGroup>? acceptedTypeGroups,
    String? initialDirectory,
    String? confirmButtonText,
  }) async => [XFile(uploadPath)];
  @override
  Future<FileSaveLocation?> getSaveLocation({
    List<XTypeGroup>? acceptedTypeGroups,
    SaveDialogOptions options = const SaveDialogOptions(),
  }) async => FileSaveLocation(downloadPath);
}

class Transfers extends SftpOperations {
  int lists = 0;
  final uploads = <String>[];
  final downloads = <String>[];
  final uploading = Completer<void>();
  @override
  Future<List<SftpEntry>> list(String session, String path) async {
    lists++;
    return [
      const SftpEntry(
        name: 'design.v',
        path: '/projects/design.v',
        kind: 'file',
        size: 32,
      ),
    ];
  }

  @override
  Future<void> upload(
    String session,
    String source,
    String destination, {
    bool resume = false,
  }) async {
    uploads.add(destination);
    await uploading.future;
  }

  @override
  Future<void> download(
    String session,
    String source,
    String destination, {
    bool resume = false,
  }) async {
    downloads.add(destination);
  }
}

void main() {
  testWidgets(
    'upload needs no selection; resize retains directory, selection and active transfer',
    (tester) async {
      final directory = Directory.systemTemp.createTempSync(
        'aetherdesk-sftp-test',
      );
      final file = File('${directory.path}/local.v');
      file.writeAsStringSync('module test; endmodule');
      final picker = Files()
        ..uploadPath = file.path
        ..downloadPath = '${directory.path}/saved.v';
      final previous = FileSelectorPlatform.instance;
      FileSelectorPlatform.instance = picker;
      addTearDown(() async {
        FileSelectorPlatform.instance = previous;
        directory.deleteSync(recursive: true);
      });
      final transfers = Transfers();
      Future<void> layout(bool horizontal, {bool visible = true}) =>
          tester.pumpWidget(
            MaterialApp(
              home: Scaffold(
                body: Align(
                  alignment: Alignment.topLeft,
                  child: SizedBox(
                    width: horizontal ? 1600 : 800,
                    child: WorkspaceSplit(
                      explorerVisible: visible,
                      explorer: SftpExplorer(
                        sessionId: 'test',
                        terminalDirectory: '.',
                        operations: transfers,
                        initialLocalDirectory: directory.path,
                      ),
                      terminal: const Placeholder(key: ValueKey('terminal')),
                    ),
                  ),
                ),
              ),
            ),
          );
      tester.view.physicalSize = const Size(1600, 1000);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await layout(true);
      await tester.pumpAndSettle();
      final upload = find.widgetWithIcon(IconButton, Icons.upload_rounded);
      expect(tester.widget<IconButton>(upload).onPressed, isNotNull);
      await tester.tap(upload);
      await tester.pump();
      await tester.pump();
      expect(transfers.uploads, ['./local.v']);
      await tester.tap(find.text('design.v'));
      await tester.pump(const Duration(milliseconds: 400));
      final selected = find.widgetWithIcon(IconButton, Icons.download_rounded);
      expect(tester.widget<IconButton>(selected).onPressed, isNotNull);
      await layout(false);
      await tester.pump();
      expect(transfers.lists, 1);
      expect(tester.widget<IconButton>(selected).onPressed, isNotNull);
      expect(find.textContaining('1 active'), findsOneWidget);
      await layout(false, visible: false);
      await tester.pump();
      expect(
        tester.getSize(find.byKey(const ValueKey('terminal'))).height,
        1000,
      );
      expect(tester.takeException(), isNull);
      await layout(true);
      await tester.pump();
      expect(transfers.lists, 1);
      expect(tester.widget<IconButton>(selected).onPressed, isNotNull);
      expect(find.textContaining('1 active'), findsOneWidget);
      await tester.runAsync(() async {
        await tester.tap(selected);
        await Future<void>.delayed(const Duration(milliseconds: 50));
      });
      await tester.pump();
      await tester.pump();
      expect(transfers.downloads, [picker.downloadPath]);
      transfers.uploading.complete();
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 50)),
      );
      await tester.pumpAndSettle();
      await tester.pumpWidget(const SizedBox());
    },
  );
}
