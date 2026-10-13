import 'dart:async';
import 'dart:io';

import 'package:ad_remote_desktop/editor/editor_workspace.dart';
import 'package:ad_remote_desktop/file_workspace/file_open_controller.dart';
import 'package:ad_remote_desktop/file_workspace/document_session.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

class _Files extends FileOpenController {
  _Files()
    : super(
        identity: 'editor-shutdown-test',
        execute: (request, {job, onProgress}) async => {},
      );

  int flushes = 0;

  @override
  Future<void> flushAll() async {
    flushes++;
  }
}

void main() {
  test(
    'window shutdown waits for an in-flight native initialization',
    () async {
      final lifecycle = EditorResourceLifecycle();
      final initialized = Completer<void>();
      final disposed = Completer<void>();
      final events = <String>[];
      final initializing = lifecycle.initialize(() async {
        events.add('creating');
        await initialized.future;
        events.add('created');
      });
      var shutdownDone = false;
      final closing = lifecycle
          .shutdown(() async {
            events.add('disposing');
            await disposed.future;
            events.add('disposed');
          })
          .then((_) => shutdownDone = true);

      await Future<void>.delayed(Duration.zero);
      expect(events, ['creating']);
      expect(lifecycle.stopping, true);
      expect(shutdownDone, false);
      initialized.complete();
      await initializing;
      await Future<void>.delayed(Duration.zero);
      expect(events, ['creating', 'created', 'disposing']);
      expect(shutdownDone, false);
      disposed.complete();
      await closing;
      expect(shutdownDone, true);
      expect(events.last, 'disposed');
    },
  );

  test(
    'a native initialization failure still releases other resources',
    () async {
      final lifecycle = EditorResourceLifecycle();
      final initialized = Completer<void>();
      final initializing = lifecycle.initialize(() => initialized.future);
      final error = expectLater(initializing, throwsStateError);
      var released = false;
      final closing = lifecycle.shutdown(() async => released = true);
      initialized.completeError(StateError('WebView2 could not initialize'));
      await error;
      await closing;
      expect(released, true);
    },
  );

  test('shutdown failure cannot authorize native closure', () async {
    final lifecycle = EditorResourceLifecycle();
    var attempts = 0;
    Future<void> release() async {
      if (++attempts == 1) throw StateError('dispose failed');
    }

    await expectLater(lifecycle.shutdown(release), throwsStateError);
    expect(lifecycle.stopping, true);
    // Failed cleanup is not recorded as a successful shutdown. A caller must
    // obtain an acknowledged release before it can destroy the window engine.
    await lifecycle.shutdown(release);
    await lifecycle.shutdown(release);
    expect(attempts, 2);
  });

  test(
    'repeated close requests share one disposal and prevent late startup',
    () async {
      final lifecycle = EditorResourceLifecycle();
      final disposed = Completer<void>();
      var releases = 0;
      Future<void> release() async {
        releases++;
        await disposed.future;
      }

      final first = lifecycle.shutdown(release);
      final second = lifecycle.shutdown(release);
      expect(identical(first, second), true);
      await lifecycle.initialize(() async => fail('Created after closing'));
      expect(releases, 1);
      disposed.complete();
      await Future.wait([first, second]);
      await lifecycle.shutdown(release);
      expect(releases, 1);
    },
  );

  testWidgets('empty editor shuts down without WebView and remains flushable', (
    tester,
  ) async {
    final files = _Files();
    final view = EditorWorkspaceController();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: EditorWorkspace(
            controller: files,
            viewController: view,
            sessionId: 'test',
            onSave: (_) async {},
            onClose: (_) async {},
          ),
        ),
      ),
    );
    expect(find.text('Double-click a file to open it.'), findsOneWidget);
    final closing = view.shutdown();
    await tester.pumpAndSettle();
    await closing;
    expect(find.text('Double-click a file to open it.'), findsNothing);
    await view.shutdown();
    await view.flush();
    expect(files.flushes, 1);
    await tester.pumpWidget(const SizedBox.shrink());
    files.dispose();
    expect(tester.takeException(), isNull);
  });

  for (final state in [AppLifecycleState.hidden, AppLifecycleState.paused]) {
    testWidgets('shutdown completes while frames are disabled: $state', (
      tester,
    ) async {
      final files = _Files();
      final view = EditorWorkspaceController();
      Future<void> lifecycle(AppLifecycleState value) async {
        await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          'flutter/lifecycle',
          const StringCodec().encodeMessage(value.toString()),
          (_) {},
        );
      }

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: EditorWorkspace(
              controller: files,
              viewController: view,
              sessionId: 'test',
              onSave: (_) async {},
              onClose: (_) async {},
            ),
          ),
        ),
      );
      await lifecycle(state);
      expect(tester.binding.framesEnabled, false);
      var completed = false;
      final closing = view.shutdown().then((_) => completed = true);
      // Flush microtasks without pumping a normal frame. Shutdown itself must
      // request the warm-up frame needed to unmount the editor while hidden.
      await tester.idle();
      expect(completed, true);
      await closing;
      expect(tester.binding.framesEnabled, false);
      expect(find.text('Double-click a file to open it.'), findsNothing);
      await lifecycle(AppLifecycleState.resumed);
      await tester.pumpWidget(const SizedBox.shrink());
      files.dispose();
    });
  }

  testWidgets(
    'removing the widget retains the pending native disposal barrier',
    (tester) async {
      final files = _Files();
      final view = EditorWorkspaceController();
      final initializeCalled = Completer<void>();
      final initializeReply = Completer<Map<String, Object?>>();
      final disposeCalled = Completer<void>();
      final disposeReply = Completer<void>();
      const plugin = MethodChannel('io.jns.webview.win');
      const events = MethodChannel('io.jns.webview.win/41/events');
      final messenger = tester.binding.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(plugin, (call) async {
        switch (call.method) {
          case 'getWebViewVersion':
            return 'test-runtime';
          case 'initialize':
            initializeCalled.complete();
            return initializeReply.future;
          case 'dispose':
            expect(call.arguments, 41);
            disposeCalled.complete();
            await disposeReply.future;
            return null;
          default:
            fail('Unexpected WebView method: ${call.method}');
        }
      });
      messenger.setMockMethodCallHandler(events, (_) async => null);
      addTearDown(() {
        messenger.setMockMethodCallHandler(plugin, null);
        messenger.setMockMethodCallHandler(events, null);
      });
      files.documents.add(
        DocumentSession(
          id: 'doc',
          remotePath: '/file.txt',
          localFile: File('unused-working-copy.txt'),
          metadata: {},
        )..editorVisible = true,
      );
      files.selectedId = 'doc';
      Future<void> settleIoUntil(bool Function() complete) async {
        for (var attempt = 0; attempt < 100 && !complete(); attempt++) {
          await tester.runAsync(
            () => Future<void>.delayed(const Duration(milliseconds: 10)),
          );
          await tester.pump();
        }
        expect(
          complete(),
          true,
          reason: 'Expected the mocked native operation to complete',
        );
      }

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: EditorWorkspace(
              controller: files,
              viewController: view,
              sessionId: 'test',
              onSave: (_) async {},
              onClose: (_) async {},
            ),
          ),
        ),
      );
      await settleIoUntil(() => initializeCalled.isCompleted);
      await tester.pumpWidget(const SizedBox.shrink());
      var closed = false;
      final closing = view.shutdown().then((_) => closed = true);
      await tester.idle();
      expect(closed, false);
      initializeReply.complete({'textureId': 41});
      await settleIoUntil(() => disposeCalled.isCompleted);
      expect(closed, false);
      disposeReply.complete();
      await settleIoUntil(() => closed);
      await closing;
      expect(closed, true);
      files.dispose();
    },
    skip: !Platform.isWindows,
  );
}
