import 'dart:async';
import 'dart:io';

import 'package:ad_remote_desktop/native_bridge.dart';
import 'package:ad_remote_desktop/x11_server.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test('local cookies cannot truncate the Windows Xauthority reader', () {
    final cookies = <String>{};
    for (var index = 0; index < 256; index++) {
      final cookie = X11SetupHandshake.createCookie();
      expect(cookie, matches(RegExp(r'^[0-9a-f]{32}$')));
      final packet = X11SetupHandshake.packet(cookie);
      expect(packet.sublist(packet.length - 16), isNot(contains(0x1a)));
      cookies.add(cookie);
    }
    expect(cookies.length, 256);
  });
  test('X11 is resolved only from the complete private application bundle', () {
    final package = Directory.systemTemp.createTempSync('aetherdesk bundle ');
    addTearDown(() => package.deleteSync(recursive: true));
    final missingBundle = isA<X11ServerException>().having(
      (error) => error.message,
      'repair instructions',
      allOf(
        contains('Reinstall AetherDesk'),
        isNot(contains('Install VcXsrv')),
      ),
    );
    // This must fail even on a machine with VcXsrv installed in Program Files.
    expect(
      () => X11ServerManager.bundledExecutable(package),
      throwsA(missingBundle),
    );
    for (final relative in [
      'vcxsrv.exe',
      'xauth.exe',
      'xkbcomp.exe',
      'fonts/misc/fonts.dir',
      'xkbdata/rules/base',
    ]) {
      final file = File('${package.path}/x11/$relative');
      file.parent.createSync(recursive: true);
      file.writeAsBytesSync([]);
    }
    expect(
      X11ServerManager.bundledExecutable(package).resolveSymbolicLinksSync(),
      File('${package.path}/x11/vcxsrv.exe').resolveSymbolicLinksSync(),
    );
    File('${package.path}/x11/xauth.exe').deleteSync();
    expect(
      () => X11ServerManager.bundledExecutable(package),
      throwsA(missingBundle),
    );
  });
  test('fallback preferences leave application icons intact', () {
    expect(
      X11ServerManager.preferences(r'C:\Aether Desk\x11'),
      'ICONDirectory "C:\\Aether Desk\\x11"\nDEFAULTICON "aetherdesk.ico"\n',
    );
    expect(
      () => X11ServerManager.preferences('bad\npath'),
      throwsFormatException,
    );
  });
  test(
    'display lifetime follows process exit and drains its log pipes',
    () async {
      final process = _TestProcess();
      final session = X11ServerSession(
        process: process,
        displayNumber: 10,
        authorizationFile: File('unused-xauthority'),
        options: const X11ConnectionOptions(
          localPort: 6010,
          authenticationProtocol: 'MIT-MAGIC-COOKIE-1',
          authenticationCookie: '00112233445566778899aabbccddeeff',
        ),
      );
      expect(session.isRunning, isTrue);
      expect(session.exitCode, isNull);
      expect(process.output.hasListener, isTrue);
      expect(process.errors.hasListener, isTrue);
      process.output.add(List.filled(128 * 1024, 65));
      process.errors.add(List.filled(128 * 1024, 66));
      process.finished.complete(7);
      expect(await session.exited, 7);
      expect(session.isRunning, isFalse);
      expect(session.exitCode, 7);
      await process.output.close();
      await process.errors.close();
    },
  );

  // Opt in with AETHERDESK_X11_PACKAGE pointing to a package made by Bundle-X11.
  // This uses an isolated display, never a system X server or a remote host.
  test(
    'busy X11 display survives health checks and reuse, actual exit is detected',
    () async {
      final package = Directory(
        Platform.environment['AETHERDESK_X11_PACKAGE']!,
      );
      final manager = X11ServerManager(applicationDirectory: package);
      Socket? socket;
      try {
        final starts = await Future.wait([
          manager.ensureRunning(),
          manager.ensureRunning(),
        ]);
        final session = starts.first;
        expect(identical(session, starts.last), isTrue);
        // The startup probe has disconnected. Allow the internal WM's five
        // second retry window to pass before keeping any test client open.
        // Previously a last-client reset spawned competing WM generations.
        await Future<void>.delayed(const Duration(seconds: 7));
        expect(await manager.isRunning(), isTrue);
        final otherManager = X11ServerManager(applicationDirectory: package);
        try {
          final other = await otherManager.ensureRunning();
          expect(other.displayNumber, isNot(session.displayNumber));
          expect(await manager.isRunning(), isTrue);
        } finally {
          await otherManager.stop();
        }
        socket = await Socket.connect('127.0.0.1', session.options.localPort);
        final client = _XClient(socket);
        socket.add(
          X11SetupHandshake.packet(session.options.authenticationCookie),
        );
        await socket.flush();
        final prefix = await client.read(8);
        expect(prefix.first, 1);
        await client.read((prefix[6] | (prefix[7] << 8)) * 4);

        socket.add([36, 0, 1, 0]); // XGrabServer, as used by X11 applications.
        await client.sync();
        for (var check = 0; check < 4; check++) {
          await Future<void>.delayed(const Duration(milliseconds: 1100));
          expect(await manager.isRunning(), isTrue);
          expect(identical(await manager.ensureRunning(), session), isTrue);
          await client.sync();
        }
        socket.add([37, 0, 1, 0]); // XUngrabServer.
        await client.sync();
        expect(await manager.isRunning(), isTrue);

        session.process.kill();
        await session.exited.timeout(const Duration(seconds: 5));
        expect(await manager.isRunning(), isFalse);
        expect(manager.exitCode, isNotNull);
        socket.destroy();
        socket = null;
        await manager.stop();
        expect(await session.authorizationFile.exists(), isFalse);
        final restarted = await manager.ensureRunning();
        expect(restarted.process.pid, isNot(session.process.pid));
        await Future<void>.delayed(const Duration(seconds: 7));
        expect(await manager.isRunning(), isTrue);
        final stopping = manager.stop();
        final reconnecting = manager.ensureRunning();
        await stopping;
        expect(await restarted.authorizationFile.exists(), isFalse);
        final reconnected = await reconnecting;
        expect(reconnected.process.pid, isNot(restarted.process.pid));
        expect(reconnected.isRunning, isTrue);
        final log = await File(
          '${reconnected.authorizationFile.parent.path}${Platform.pathSeparator}vcxsrv-${reconnected.displayNumber}.log',
        ).readAsString();
        expect(log, contains('.XWinrc'));
        expect(log, isNot(contains('badly formed')));
        expect(log, isNot(contains('winOverrideDefaultIcon: LoadImageComma')));
        await manager.stop();
        expect(await manager.isRunning(), isFalse);
      } finally {
        socket?.destroy();
        await manager.stop();
      }
    },
    skip:
        !Platform.isWindows ||
        Platform.environment['AETHERDESK_X11_INTEGRATION'] != '1',
  );

  test('builds an aligned X11 setup packet with a decoded cookie', () {
    final packet = X11SetupHandshake.packet('00112233445566778899aabbccddeeff');

    expect(packet[0], 0x6c);
    expect(packet.sublist(2, 4), [11, 0]);
    expect(packet[6], X11SetupHandshake.authenticationProtocol.length);
    expect(packet[8], 16);
    expect(packet.length % 4, 0);
    expect(packet.sublist(packet.length - 16), [
      0x00,
      0x11,
      0x22,
      0x33,
      0x44,
      0x55,
      0x66,
      0x77,
      0x88,
      0x99,
      0xaa,
      0xbb,
      0xcc,
      0xdd,
      0xee,
      0xff,
    ]);
  });

  test('rejects malformed X11 cookies', () {
    expect(() => X11SetupHandshake.packet('abc'), throwsFormatException);
    expect(() => X11SetupHandshake.packet('zz'), throwsFormatException);
  });

  test('distinguishes X11 setup success from authorization failure', () {
    expect(
      X11SetupHandshake.rejectionReason([1, 0, 11, 0, 0, 0, 0, 0]),
      isNull,
    );
    expect(
      X11SetupHandshake.rejectionReason([0, 12, 11, 0, 0, 0, 0, 0]),
      contains('rejected'),
    );
    expect(X11SetupHandshake.rejectionReason([1]), contains('truncated'));
  });
}

class _TestProcess implements Process {
  final finished = Completer<int>();
  final output = StreamController<List<int>>();
  final errors = StreamController<List<int>>();

  @override
  Future<int> get exitCode => finished.future;
  @override
  Stream<List<int>> get stdout => output.stream;
  @override
  Stream<List<int>> get stderr => errors.stream;
  @override
  IOSink get stdin => throw UnimplementedError();
  @override
  int get pid => 1;
  @override
  bool kill([ProcessSignal signal = ProcessSignal.sigterm]) => false;
}

class _XClient {
  _XClient(this.socket) : chunks = StreamIterator(socket);

  final Socket socket;
  final StreamIterator<List<int>> chunks;
  final pending = <int>[];

  Future<List<int>> read(int length) async {
    while (pending.length < length) {
      if (!await chunks.moveNext().timeout(const Duration(seconds: 5))) {
        throw StateError('X server closed the test connection');
      }
      pending.addAll(chunks.current);
    }
    final result = pending.sublist(0, length);
    pending.removeRange(0, length);
    return result;
  }

  Future<void> sync() async {
    socket.add([43, 0, 1, 0]); // GetInputFocus proves prior requests ran.
    await socket.flush();
    final reply = await read(32);
    expect(reply.first, 1);
  }
}
