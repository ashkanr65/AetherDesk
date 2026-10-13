import 'dart:async';
import 'dart:io';

import 'package:ad_remote_desktop/editor/editor_workspace.dart';
import 'package:ad_remote_desktop/file_workspace/document_session.dart';
import 'package:ad_remote_desktop/file_workspace/file_context_menu.dart';
import 'package:ad_remote_desktop/file_workspace/file_open_controller.dart';
import 'package:ad_remote_desktop/native_bridge.dart';
import 'package:ad_remote_desktop/workspace.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'transfer_queue_test.dart' show FakeHost;

class WorkspaceListing extends SftpOperations {
  WorkspaceListing(this.host);
  final FakeHost host;
  final paths = <String>[];
  String? failingPath;
  final pending = <String, Completer<List<SftpEntry>>>{};
  @override
  Future<List<SftpEntry>> list(String session, String path) async {
    paths.add(path);
    if (path == failingPath) throw StateError('Folder unavailable');
    if (pending.containsKey(path)) return pending[path]!.future;
    final reply = await host.execute({'operation': 'list', 'path': path});
    return [
      for (final entry in reply['entries'] as List)
        SftpEntry(
          name: (entry as Map)['name'] as String,
          path: '$path/${entry['name']}',
          kind: host.directories.contains('$path/${entry['name']}')
              ? 'directory'
              : 'file',
          size: host.files['$path/${entry['name']}']?.length,
        ),
    ];
  }
}

class RecordingActions extends FileWorkspaceActions {
  RecordingActions({required super.context, required super.controller})
    : super(
        editor: EditorWorkspaceController(),
        showEditor: () {},
        refresh: () async {},
      );
  final opened = <String>[];
  final actionCompleted = Completer<void>();
  @override
  Future<void> run(Future<void> Function() action) async {
    try {
      await super.run(action);
    } finally {
      if (!actionCompleted.isCompleted) actionCompleted.complete();
    }
  }

  @override
  Future<void> open(
    String path, {
    required bool remote,
    bool forceEditor = false,
    bool system = false,
    String? application,
  }) async {
    opened.add('$remote:$path:$forceEditor:$system');
  }
}

void main() {
  late Directory local;
  late FakeHost host;
  late WorkspaceListing listing;
  late FileOpenController controller;
  late RecordingActions actions;
  final inserted = <String>[];
  setUp(() async {
    local = await Directory.systemTemp.createTemp('aetherdesk-workspace-ui-');
    await File('${local.path}/local.txt').writeAsString('local');
    await Directory('${local.path}/local-folder').create();
    await File(
      '${local.path}/local-folder/child.txt',
    ).writeAsString('uploaded');
    host = FakeHost();
    host.files['/host/design.v'] = 'module test; endmodule';
    host.directories.add('/host/remote-folder');
    host.files['/host/remote-folder/from-host.txt'] = 'downloaded';
    listing = WorkspaceListing(host);
    controller = FileOpenController(
      identity: 'test',
      execute: host.execute,
      cache: Directory('${local.path}/cache'),
    );
    inserted.clear();
  });
  tearDown(() async {
    // Finish journals before deleting the fixture directory on Windows.
    for (final doc in List<DocumentSession>.of(controller.documents)) {
      await controller.close(doc);
    }
    controller.dispose();
    await local.delete(recursive: true);
  });

  Future<void> mount(WidgetTester tester, {bool terminal = true}) async {
    tester.view.physicalSize = const Size(1600, 1200);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.runAsync(() async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) {
                actions = RecordingActions(
                  context: () => context,
                  controller: controller,
                );
                return SftpExplorer(
                  sessionId: 'test',
                  terminalDirectory: '/host',
                  initialLocalDirectory: local.path,
                  operations: listing,
                  actions: actions,
                  onInsertPath: terminal ? inserted.add : null,
                );
              },
            ),
          ),
        ),
      );
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await tester.pumpAndSettle();
  }

  Future<void> rightClick(WidgetTester tester, String name) async {
    await tester.tap(find.text(name), buttons: kSecondaryMouseButton);
    await tester.pumpAndSettle();
  }

  testWidgets(
    'initial host directory and double-click open use file workspace',
    (tester) async {
      await mount(tester);
      expect(listing.paths.first, '/host');
      await tester.tap(find.text('design.v'));
      await tester.pump(const Duration(milliseconds: 60));
      await tester.tap(find.text('design.v'));
      await tester.pumpAndSettle();
      expect(actions.opened, ['true:/host/design.v:false:false']);
      await tester.tap(find.text('local.txt'));
      await tester.pump(const Duration(milliseconds: 60));
      await tester.tap(find.text('local.txt'));
      await tester.pumpAndSettle();
      expect(actions.opened.last, contains('false:'));
      expect(actions.opened.last, contains('local.txt'));
    },
  );

  testWidgets(
    'file menu has editor, application and compare actions but no Download',
    (tester) async {
      await mount(tester);
      await rightClick(tester, 'design.v');
      for (final label in [
        'Open',
        'Open with built-in editor',
        'Open with…',
        'Open with default program',
        'Compare file with…',
        'Copy file path',
        'Copy file path to terminal',
        'Properties',
        'Permissions',
      ]) {
        expect(find.text(label), findsOneWidget);
      }
      expect(find.text('Download'), findsNothing);
      await tester.tap(find.text('Open with built-in editor'));
      await tester.pumpAndSettle();
      expect(actions.opened, ['true:/host/design.v:true:false']);
    },
  );

  testWidgets('folder menu omits file-only actions and Open navigates', (
    tester,
  ) async {
    await mount(tester);
    await rightClick(tester, 'remote-folder');
    expect(find.text('Open with built-in editor'), findsNothing);
    expect(find.text('Compare file with…'), findsNothing);
    expect(find.text('Download'), findsNothing);
    for (final label in [
      'Rename',
      'Delete',
      'Copy file path',
      'Properties',
      'Permissions',
    ]) {
      expect(find.text(label), findsOneWidget);
    }
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(listing.paths.last, '/host/remote-folder');
    expect(find.text('from-host.txt'), findsOneWidget);
  });

  testWidgets('a local folder containing an open document cannot be renamed', (
    tester,
  ) async {
    await mount(tester);
    final file = File('${local.path}/local-folder/child.txt');
    final openedPath = Platform.isWindows
        ? file.path.replaceAll('/', r'\')
        : file.path;
    await tester.runAsync(() async {
      // Match real document opening: Windows CI can use a short-name alias for
      // its temp directory while the folder guard resolves the full path.
      final doc = await controller.openLocal(openedPath);
      if (Platform.isWindows) {
        // Keep coverage for case and separator differences after resolution.
        doc.remotePath = doc.remotePath.replaceAll(r'\', '/').toUpperCase();
      }
    });
    await tester.runAsync(() async {
      await rightClick(tester, 'local-folder');
      await tester.tap(find.text('Rename'));
      await actions.actionCompleted.future.timeout(const Duration(seconds: 10));
    });
    await tester.pumpAndSettle();
    expect(
      find.textContaining('Close this file or its open child files'),
      findsOneWidget,
    );
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('relative host paths protect their canonical open documents', (
    tester,
  ) async {
    await mount(tester);
    host.files['./design.v'] = host.files['/host/design.v']!;
    host.canonicalPaths['./design.v'] = '/host/design.v';
    controller.documents.add(
      DocumentSession(
        id: 'remote-open',
        remotePath: '/host/design.v',
        localFile: File('${local.path}/working.v'),
        metadata: {},
      ),
    );
    unawaited(
      actions.menu(
        position: const Offset(500, 150),
        path: './design.v',
        remote: true,
        directory: false,
        openFolder: () {},
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Rename'));
    await tester.pumpAndSettle();
    expect(
      find.textContaining('Close this file or its open child files'),
      findsOneWidget,
    );
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets('deleting a host link does not act on its open target', (
    tester,
  ) async {
    await mount(tester);
    host.links.add('./shortcut');
    host.canonicalPaths['./shortcut'] = '/host/design.v';
    controller.documents.add(
      DocumentSession(
        id: 'remote-open',
        remotePath: '/host/design.v',
        localFile: File('${local.path}/working.v'),
        metadata: {},
      ),
    );
    unawaited(
      actions.menu(
        position: const Offset(500, 150),
        path: './shortcut',
        remote: true,
        directory: false,
        openFolder: () {},
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(find.text('Delete file?'), findsOneWidget);
    expect(
      find.textContaining('Close this file or its open child files'),
      findsNothing,
    );
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(host.calls.where((call) => call['operation'] == 'delete'), isEmpty);
  });

  testWidgets('SFTP-only menus omit terminal insertion', (tester) async {
    await mount(tester, terminal: false);
    await rightClick(tester, 'design.v');
    expect(find.text('Copy file path to terminal'), findsNothing);
  });

  testWidgets(
    'middle-click inserts a quoted host path without opening a file',
    (tester) async {
      await mount(tester);
      await tester.tap(find.text('design.v'), buttons: kMiddleMouseButton);
      await tester.pumpAndSettle();
      expect(inserted, ["'/host/design.v'"]);
      expect(actions.opened, isEmpty);
    },
  );

  testWidgets('local folders drag to host through recursive managed transfer', (
    tester,
  ) async {
    await mount(tester);
    final source = tester.getCenter(find.text('local-folder'));
    final target = tester.getCenter(
      find.byWidgetPredicate(
        (widget) => widget is DragTarget<FileSystemEntity>,
      ),
    );
    await tester.runAsync(() async {
      await tester.dragFrom(source, target - source);
      await Future<void>.delayed(const Duration(milliseconds: 150));
    });
    await tester.pumpAndSettle();
    expect(host.files['/host/local-folder/child.txt'], 'uploaded');
    expect(find.textContaining('Completed'), findsOneWidget);
  });

  testWidgets('host folders drag to local pane with nested contents', (
    tester,
  ) async {
    await mount(tester);
    final source = tester.getCenter(find.text('remote-folder'));
    final target = tester.getCenter(
      find.byWidgetPredicate((widget) => widget is DragTarget<SftpEntry>),
    );
    await tester.runAsync(() async {
      await tester.dragFrom(source, target - source);
      await Future<void>.delayed(const Duration(milliseconds: 150));
    });
    await tester.pumpAndSettle();
    expect(
      await tester.runAsync(
        () => File('${local.path}/remote-folder/from-host.txt').readAsString(),
      ),
      'downloaded',
    );
    expect(find.textContaining('Completed'), findsOneWidget);
  });
  final localAddress = find.byKey(const ValueKey('Local Files address'));
  final remoteAddress = find.byKey(const ValueKey('Remote Files address'));

  Future<void> submitAddress(
    WidgetTester tester,
    Finder field,
    String path,
  ) async {
    await tester.enterText(field, path);
    await tester.runAsync(() async {
      await tester.testTextInput.receiveAction(TextInputAction.go);
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await tester.pumpAndSettle();
  }

  for (final terminal in [true, false]) {
    testWidgets('editable addresses navigate both panes (terminal=$terminal)', (
      tester,
    ) async {
      await mount(tester, terminal: terminal);
      expect(tester.widget<TextField>(localAddress).readOnly, isFalse);
      expect(tester.widget<TextField>(remoteAddress).controller!.text, '/host');
      await tester.enterText(remoteAddress, '/host/remote-folder');
      expect(listing.paths, ['/host']); // Editing alone never sends a command.
      await submitAddress(tester, remoteAddress, '/host/remote-folder');
      expect(find.text('from-host.txt'), findsOneWidget);
      expect(find.text('design.v'), findsNothing);
      await submitAddress(tester, localAddress, 'local-folder');
      expect(find.text('child.txt'), findsOneWidget);
      expect(find.text('local.txt'), findsNothing);
      expect(
        Directory(
          tester.widget<TextField>(localAddress).controller!.text,
        ).absolute.uri,
        Directory('${local.path}/local-folder').absolute.uri,
      );
      await submitAddress(tester, localAddress, local.path);
      expect(find.text('local.txt'), findsOneWidget);
    });
  }

  testWidgets('addresses support select all, copy, paste and Escape', (
    tester,
  ) async {
    String? clipboard;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          clipboard = (call.arguments as Map)['text'] as String;
          return null;
        }
        if (call.method == 'Clipboard.getData') return {'text': clipboard};
        if (call.method == 'Clipboard.hasStrings')
          return {'value': clipboard?.isNotEmpty ?? false};
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await mount(tester);
    for (final field in [localAddress, remoteAddress]) {
      final original = tester.widget<TextField>(field).controller!.text;
      await tester.tap(field);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pumpAndSettle();
      expect((await Clipboard.getData(Clipboard.kTextPlain))!.text, original);
      await Clipboard.setData(const ClipboardData(text: '/pasted folder'));
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pumpAndSettle();
      expect(
        tester.widget<TextField>(field).controller!.text,
        '/pasted folder',
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(tester.widget<TextField>(field).controller!.text, original);
    }
    expect(listing.paths, ['/host']);
  });

  testWidgets('bad addresses preserve the open folders and their contents', (
    tester,
  ) async {
    await mount(tester);
    listing.failingPath = '/missing';
    await submitAddress(tester, remoteAddress, '/missing');
    expect(find.text('design.v'), findsOneWidget);
    expect(find.textContaining('Folder unavailable'), findsOneWidget);
    await tester.tap(remoteAddress);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    expect(tester.widget<TextField>(remoteAddress).controller!.text, '/host');
    await submitAddress(tester, localAddress, '${local.path}/missing');
    expect(find.text('local.txt'), findsOneWidget);
    await tester.tap(localAddress);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    expect(tester.widget<TextField>(localAddress).controller!.text, local.path);
  });

  testWidgets('a delayed listing cannot overwrite a newer address', (
    tester,
  ) async {
    await mount(tester);
    final slow = Completer<List<SftpEntry>>();
    listing.pending['/slow'] = slow;
    await tester.enterText(remoteAddress, '/slow');
    await tester.testTextInput.receiveAction(TextInputAction.go);
    await tester.pump();
    await submitAddress(tester, remoteAddress, '/host/remote-folder');
    slow.completeError(StateError('stale failure'));
    await tester.pumpAndSettle();
    expect(
      tester.widget<TextField>(remoteAddress).controller!.text,
      '/host/remote-folder',
    );
    expect(find.text('from-host.txt'), findsOneWidget);
    expect(find.textContaining('stale failure'), findsNothing);
  });
}
