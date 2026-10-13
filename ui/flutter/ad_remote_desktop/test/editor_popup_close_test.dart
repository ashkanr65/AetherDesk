import 'dart:async';
import 'package:ad_remote_desktop/editor/editor_popup_close.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'native close waits for pending editor teardown and releases once',
    () async {
      final webviewReleased = Completer<void>();
      final events = <String>[];
      final close = EditorPopupClose(
        shutdownEditor: () async {
          events.add('shutdown started');
          await webviewReleased.future;
          events.add('webview released');
        },
        closeNative: () async {
          events.add('native close');
        },
      );
      final first = close.close();
      final repeatedWindowMessage = close.close();
      await Future<void>.delayed(Duration.zero);
      expect(events, ['shutdown started']);
      webviewReleased.complete();
      await Future.wait([first, repeatedWindowMessage]);
      expect(events, ['shutdown started', 'webview released', 'native close']);
      await close.close();
      expect(events.length, 3);
    },
  );

  test(
    'failed webview disposal never permits native engine destruction',
    () async {
      var nativeCloses = 0;
      var shutdowns = 0;
      final close = EditorPopupClose(
        shutdownEditor: () async {
          shutdowns++;
          throw StateError('native disposal failed');
        },
        closeNative: () async {
          nativeCloses++;
        },
      );
      await expectLater(close.close(), throwsStateError);
      await expectLater(close.close(), throwsStateError);
      expect(shutdowns, 1);
      expect(nativeCloses, 0);
    },
  );

  test(
    'session disconnect can acknowledge completed cleanup before native close',
    () async {
      final events = <String>[];
      final close = EditorPopupClose(
        shutdownEditor: () async {
          events.add('shutdown');
        },
        closeNative: () async {
          events.add('native close');
        },
      );
      await close.prepare();
      events.add('disconnect acknowledged');
      await close.close();
      expect(events, ['shutdown', 'disconnect acknowledged', 'native close']);
    },
  );
}
