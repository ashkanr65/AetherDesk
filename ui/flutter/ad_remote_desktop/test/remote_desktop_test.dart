import 'dart:async';
import 'dart:io';

import 'package:ad_remote_desktop/remote_desktop.dart';
import 'package:ad_remote_desktop/remote_desktop_view.dart';
import 'package:ad_remote_desktop/session_store.dart';
import 'package:flutter/services.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('saved desktop identity survives app restart and profile rename', () {
    const profile = SavedSession(
      id: 'stable-profile',
      name: 'Work',
      host: 'Host',
      port: 22,
      username: 'AR',
    );
    final id = persistentDesktopId(profile, 'AR');
    expect(
      persistentDesktopId(SavedSession.fromJson(profile.toJson()), 'AR'),
      id,
    );
    expect(persistentDesktopId(profile.copyWith(name: 'Renamed'), 'AR'), id);
    expect(
      persistentDesktopId(profile.copyWith(host: 'another-host'), 'AR'),
      isNot(id),
    );
    expect(persistentDesktopId(profile, 'another-user'), isNot(id));
    expect(
      id,
      matches(
        RegExp(
          r'^[0-9a-f]{8}-[0-9a-f]{4}-8[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$',
        ),
      ),
    );
  });
  testWidgets(
    'fullscreen and all-monitor buttons select the native presentation mode',
    (tester) async {
      final calls = <MethodCall>[];
      const channel = MethodChannel('aetherdesk/stream_surface');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        calls.add(call);
        return null;
      });
      final session = _FakeSession();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: RemoteDesktopView(session: session)),
        ),
      );
      await tester.tap(find.byTooltip('Full screen'));
      await tester.pump();
      expect(calls.where((c) => c.method == 'present').last.arguments, {
        'textureId': 42,
        'allMonitors': false,
      });
      await tester.tap(find.byTooltip('Use all monitors'));
      await tester.pump();
      expect(calls.where((c) => c.method == 'present').last.arguments, {
        'textureId': 42,
        'allMonitors': true,
      });
      await tester.pumpWidget(const SizedBox());
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      );
    },
  );
  testWidgets('active Windows Spanish input layout is sent to the host', (
    tester,
  ) async {
    const channel = MethodChannel('aetherdesk/stream_surface');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      (call) async => call.method == 'keyboardLayout'
          ? {'layout': 'es', 'variant': ''}
          : null,
    );
    final session = _FakeSession();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: RemoteDesktopView(session: session)),
      ),
    );
    await tester.pump();
    expect(session.layouts, Platform.isWindows ? contains('es/') : isEmpty);
    await tester.pumpWidget(const SizedBox());
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      null,
    );
  });

  testWidgets('only confirmed end session terminates host work', (
    tester,
  ) async {
    final session = _FakeSession();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: RemoteDesktopView(session: session)),
      ),
    );
    Future<void> openEndDialog() async {
      expect(find.byType(PopupMenuButton<String>), findsNothing);
      await tester.tap(find.widgetWithText(TextButton, 'End remote session'));
      await tester.pumpAndSettle();
    }

    await openEndDialog();
    expect(session.disposals, isEmpty);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(session.disposals, isEmpty);
    await openEndDialog();
    await tester.tap(find.text('End session'));
    await tester.pumpAndSettle();
    expect(session.disposals, [true]);
    await tester.pumpWidget(const SizedBox());
    expect(session.disposals, [true]);
  });

  testWidgets(
    'end button waits for confirmation and shows termination failure',
    (tester) async {
      final session = _FakeSession()..ending = Completer<void>();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: RemoteDesktopView(session: session)),
        ),
      );
      await tester.tap(find.widgetWithText(TextButton, 'End remote session'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('End session'));
      await tester.pumpAndSettle();
      expect(session.disposals, [true]);
      final button = tester.widget<TextButton>(
        find.widgetWithText(TextButton, 'Ending session…'),
      );
      expect(button.onPressed, isNull);
      session.ending!.completeError(
        StateError('Host did not confirm termination'),
      );
      await tester.pumpAndSettle();
      expect(
        find.textContaining('Host did not confirm termination'),
        findsOneWidget,
      );
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'desktop shows its texture, forwards keys, and releases held keys on removal',
    (tester) async {
      final session = _FakeSession();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: RemoteDesktopView(session: session)),
        ),
      );
      expect(tester.widget<Texture>(find.byType(Texture)).textureId, 42);
      await tester.tap(find.byType(Texture));
      await tester.pump();
      await tester.sendKeyDownEvent(
        LogicalKeyboardKey.keyA,
        physicalKey: PhysicalKeyboardKey.keyA,
      );
      expect(
        session.inputs.where((event) => event.containsKey('Keyboard')).last,
        {
          'Keyboard': {
            'scan_code': 30,
            'pressed': true,
            'modifiers': 0x8000,
            'repeat': false,
          },
        },
      );
      await tester.pumpWidget(const SizedBox());
      expect(
        session.inputs.where((event) => event.containsKey('Keyboard')).last,
        {
          'Keyboard': {
            'scan_code': 30,
            'pressed': false,
            'modifiers': 0x8000,
            'repeat': false,
          },
        },
      );
      await tester.sendKeyUpEvent(
        LogicalKeyboardKey.keyA,
        physicalKey: PhysicalKeyboardKey.keyA,
      );
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'remote desktop forwards Ctrl and Alt shortcut chords to the host',
    (tester) async {
      final session = _FakeSession();
      var intercepted = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: CallbackShortcuts(
            bindings: {
              const SingleActivator(
                LogicalKeyboardKey.keyA,
                control: true,
              ): () =>
                  intercepted++,
            },
            child: Scaffold(body: RemoteDesktopView(session: session)),
          ),
        ),
      );
      await tester.tap(find.byType(Texture));
      await tester.pump();
      for (final modifier in [
        LogicalKeyboardKey.controlLeft,
        LogicalKeyboardKey.altLeft,
      ]) {
        for (final entry in {
          LogicalKeyboardKey.keyA: PhysicalKeyboardKey.keyA,
          LogicalKeyboardKey.keyE: PhysicalKeyboardKey.keyE,
          LogicalKeyboardKey.keyF: PhysicalKeyboardKey.keyF,
          LogicalKeyboardKey.keyB: PhysicalKeyboardKey.keyB,
          LogicalKeyboardKey.keyU: PhysicalKeyboardKey.keyU,
          LogicalKeyboardKey.keyK: PhysicalKeyboardKey.keyK,
          LogicalKeyboardKey.keyW: PhysicalKeyboardKey.keyW,
          LogicalKeyboardKey.keyY: PhysicalKeyboardKey.keyY,
          LogicalKeyboardKey.keyT: PhysicalKeyboardKey.keyT,
          LogicalKeyboardKey.minus: PhysicalKeyboardKey.minus,
        }.entries) {
          session.inputs.clear();
          await tester.sendKeyDownEvent(modifier);
          await tester.sendKeyEvent(entry.key, physicalKey: entry.value);
          await tester.sendKeyUpEvent(modifier);
          final keys = session.inputs
              .where((e) => e.containsKey('Keyboard'))
              .map((e) => e['Keyboard'] as Map)
              .toList();
          expect(keys.map((e) => e['scan_code']), [
            modifier == LogicalKeyboardKey.controlLeft ? 29 : 56,
            remoteScanCode(entry.value),
            remoteScanCode(entry.value),
            modifier == LogicalKeyboardKey.controlLeft ? 29 : 56,
          ]);
          expect(keys.map((e) => e['pressed']), [true, true, false, false]);
        }
      }
      expect(intercepted, 0);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('resize uses even dimensions that fit the tab', (tester) async {
    const channel = MethodChannel('aetherdesk/stream_surface');
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
      call,
    ) async {
      calls.add(call);
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(800, 600);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);
    final session = _FakeSession();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: RemoteDesktopView(session: session)),
      ),
    );
    await tester.tap(find.byTooltip('Resize remote desktop to this tab'));
    await tester.pump();
    if (Platform.isWindows) {
      expect(calls.where((c) => c.method == 'setTabSize').last.arguments, {
        'textureId': 42,
        'width': 800,
        'height': 558,
        'force': true,
      });
    } else {
      expect(session.size, const Size(800, 558));
    }
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets(
    'tab size follows physical pixels and window resizing automatically',
    (tester) async {
      tester.view.devicePixelRatio = 2;
      tester.view.physicalSize = const Size(1600, 1200);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.view.resetPhysicalSize);
      const channel = MethodChannel('aetherdesk/stream_surface');
      final calls = <MethodCall>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        calls.add(call);
        return null;
      });
      final session = _FakeSession();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: RemoteDesktopView(session: session)),
        ),
      );
      await tester.pump(const Duration(milliseconds: 200));
      if (Platform.isWindows) {
        expect(calls.where((c) => c.method == 'setTabSize').last.arguments, {
          'textureId': 42,
          'width': 1600,
          'height': 1116,
          'force': false,
        });
      } else {
        expect(session.size, const Size(1600, 1116));
      }
      tester.view.physicalSize = const Size(2000, 1400);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 200));
      if (Platform.isWindows) {
        expect(calls.where((c) => c.method == 'setTabSize').last.arguments, {
          'textureId': 42,
          'width': 2000,
          'height': 1316,
          'force': false,
        });
        final before = calls.where((c) => c.method == 'setTabSize').length;
        session.status.value = const RemoteDesktopStatus(
          width: 4800,
          height: 1800,
          frames: 2,
        );
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 200));
        expect(calls.where((c) => c.method == 'setTabSize').length, before);
      } else {
        expect(session.size, const Size(2000, 1316));
      }
      await tester.pumpWidget(const SizedBox());
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      );
    },
  );

  test('tab size respects encoder limits and even pixel dimensions', () {
    expect(remoteTabSize(const Size(801, 559), 1.5), const Size(1200, 838));
    expect(remoteTabSize(const Size(10000, 6000), 2), const Size(7200, 4320));
  });
  testWidgets('leaving the app releases held remote keys', (tester) async {
    final session = _FakeSession();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: RemoteDesktopView(session: session)),
      ),
    );
    await tester.tap(find.byType(Texture));
    await tester.pump();
    await tester.sendKeyDownEvent(
      LogicalKeyboardKey.controlLeft,
      physicalKey: PhysicalKeyboardKey.controlLeft,
    );
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    expect(
      session.inputs.where((event) => event.containsKey('Keyboard')).last,
      {
        'Keyboard': {
          'scan_code': 29,
          'pressed': false,
          'modifiers': 0x8000,
          'repeat': false,
        },
      },
    );
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.sendKeyUpEvent(
      LogicalKeyboardKey.controlLeft,
      physicalKey: PhysicalKeyboardKey.controlLeft,
    );
    await tester.pumpWidget(const SizedBox());
  });

  test('Aether Stream requires Linux, a desktop and a session bus', () {
    final ready = RemoteDesktopReadiness.parse(
      'AETHERDESK_LINUX_X64_READY\nAETHERDESK_DBUS_READY\nAETHERDESK_DESKTOP_READY\n',
    );
    expect(ready.selectedBackend, RemoteDesktopBackend.aetherStream);
    final oldRdp = RemoteDesktopReadiness.parse(
      'AETHERDESK_XRDP_READY\nAETHERDESK_DESKTOP_READY\n',
    );
    expect(oldRdp.selectedBackend, RemoteDesktopBackend.unavailable);
  });
  test('readiness markers must occupy a complete line', () {
    final result = RemoteDesktopReadiness.parse(
      'not_AETHERDESK_LINUX_X64_READY\nAETHERDESK_DBUS_READY\nAETHERDESK_DESKTOP_MISSING:startxfce4\n',
    );
    expect(result.linuxReady, isFalse);
    expect(result.missingDesktopCommand, 'startxfce4');
  });
  test('probes all offered desktops without requiring xrdp', () {
    final commands = [
      'startxfce4',
      'startplasma-x11',
      'gnome-session',
      'mate-session',
      'cinnamon-session',
      'startlxqt',
    ];
    for (final entry in DesktopEnvironment.values.indexed) {
      final command = RemoteDesktopLauncher.readinessCommand(entry.$2);
      expect(command, contains(commands[entry.$1]));
      expect(command, contains('dbus-run-session'));
      expect(command, isNot(contains('xrdp')));
    }
  });
  testWidgets('keypad sends lock state enabled before desktop focus', (
    tester,
  ) async {
    final session = _FakeSession();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: RemoteDesktopView(session: session)),
      ),
    );
    // Toggle before focusing the remote view, as when connecting with Num Lock on.
    await tester.sendKeyEvent(
      LogicalKeyboardKey.numLock,
      physicalKey: PhysicalKeyboardKey.numLock,
    );
    await tester.tap(find.byType(Texture));
    await tester.pump();
    await tester.sendKeyEvent(
      LogicalKeyboardKey.numpad1,
      physicalKey: PhysicalKeyboardKey.numpad1,
    );
    final input = session.inputs.where((e) => e.containsKey('Keyboard')).last;
    expect(input['Keyboard'], {
      'scan_code': 79,
      'pressed': false,
      'modifiers': 0x8010,
      'repeat': false,
    });
    await tester.sendKeyEvent(
      LogicalKeyboardKey.numLock,
      physicalKey: PhysicalKeyboardKey.numLock,
    );
    await tester.sendKeyEvent(
      LogicalKeyboardKey.numpad1,
      physicalKey: PhysicalKeyboardKey.numpad1,
    );
    expect((session.inputs.last['Keyboard'] as Map)['modifiers'], 0x8000);
    await tester.pumpWidget(const SizedBox());
  });
  test('lock state carries Num Lock and Caps Lock independently', () {
    expect(remoteLockModifiers({}), 0x8000);
    expect(remoteLockModifiers({KeyboardLockMode.numLock}), 0x8010);
    expect(remoteLockModifiers({KeyboardLockMode.capsLock}), 0x8020);
    expect(
      remoteLockModifiers({
        KeyboardLockMode.numLock,
        KeyboardLockMode.capsLock,
      }),
      0x8030,
    );
  });
  test('physical keys map to Linux scan codes including modifiers', () {
    expect(remoteScanCode(PhysicalKeyboardKey.keyA), 30);
    expect(remoteScanCode(PhysicalKeyboardKey.arrowLeft), 105);
    expect(remoteScanCode(PhysicalKeyboardKey.controlRight), 97);
    expect(remoteScanCode(PhysicalKeyboardKey.enter), 28);
    expect(remoteScanCode(PhysicalKeyboardKey.numpad1), 79);
    expect(remoteScanCode(PhysicalKeyboardKey.numpad9), 73);
    expect(remoteScanCode(PhysicalKeyboardKey.numpadEnter), 96);
    expect(remoteScanCode(PhysicalKeyboardKey.numLock), 69);
    expect(remoteScanCode(PhysicalKeyboardKey.audioVolumeUp), isNull);
  });
  test('pointer mapping accounts for display scaling and clamps edges', () {
    expect(
      remotePoint(
        const Offset(50, 25),
        const Size(100, 50),
        const Size(1920, 1080),
      ),
      const Offset(960, 540),
    );
    expect(
      remotePoint(
        const Offset(-10, 60),
        const Size(100, 50),
        const Size(1920, 1080),
      ),
      const Offset(0, 1079),
    );
  });
}

class _FakeSession implements RemoteDesktopSession {
  @override
  final textureId = 42;
  @override
  final ValueNotifier<RemoteDesktopStatus> status = ValueNotifier(
    const RemoteDesktopStatus(frames: 1),
  );
  final inputs = <Map<String, Object?>>[];
  Size? size;
  final layouts = <String>[];
  final disposals = <bool>[];
  Completer<void>? ending;
  @override
  Future<void> get closed => Completer<void>().future;
  @override
  void sendInput(Map<String, Object?> event) => inputs.add(event);
  @override
  void resize(int width, int height) =>
      size = Size(width.toDouble(), height.toDouble());
  @override
  void publishClipboard(String text) {}
  @override
  String? takeClipboard() => null;
  @override
  Future<void> dispose({bool terminate = false}) async {
    disposals.add(terminate);
    await ending?.future;
  }

  @override
  void setKeyboardLayout(String layout, String variant) {
    layouts.add("$layout/$variant");
  }
}
