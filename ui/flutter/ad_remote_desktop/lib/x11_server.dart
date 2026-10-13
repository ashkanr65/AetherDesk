import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/services.dart';

import 'app_log.dart';
import 'native_bridge.dart';

class X11ServerException implements Exception {
  X11ServerException(this.message) {
    // VcXsrv launch and display/cookie failures funnel through here
    // (log_feature.md §3.5).
    AppLog.instance.error(AppLogSource.x11, message);
  }

  final String message;

  @override
  String toString() => message;
}

class X11ServerSession {
  X11ServerSession({
    required this.process,
    required this.displayNumber,
    required this.authorizationFile,
    required this.options,
  }) {
    // Keep the process attached so exitCode is available. Drain both pipes:
    // otherwise a full log pipe can block VcXsrv during a long session.
    unawaited(process.stdout.drain<void>());
    unawaited(process.stderr.drain<void>());
    exited = process.exitCode.then((code) {
      _exitCode = code;
      return code;
    });
  }

  final Process process;
  final int displayNumber;
  final File authorizationFile;
  final X11ConnectionOptions options;
  late final Future<int> exited;
  int? _exitCode;

  int? get exitCode => _exitCode;
  bool get isRunning => _exitCode == null;
}

/// Builds and validates the initial X11 setup exchange used to prove that the
/// local display is usable, including its MIT-MAGIC-COOKIE-1 authorization.
/// A TCP connect alone is insufficient: an X server can accept the socket and
/// still reject every real client because its authority file is wrong.
class X11SetupHandshake {
  static const String authenticationProtocol = 'MIT-MAGIC-COOKIE-1';

  static String createCookie() {
    final random = Random.secure();
    // VcXsrv 21.1.16.1 reads Xauthority in Windows text mode: 0x1a is EOF.
    // Rejection sampling retains over 127 bits of entropy in the 16-byte key.
    final bytes = <int>[];
    while (bytes.length < 16) {
      final byte = random.nextInt(256);
      if (byte != 0x1a) bytes.add(byte);
    }
    return bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();
  }

  static List<int> packet(String hexadecimalCookie) {
    final protocol = authenticationProtocol.codeUnits;
    final cookie = _decodeHexadecimalCookie(hexadecimalCookie);
    final packet = <int>[
      0x6c, // Little-endian byte order.
      0,
      11,
      0,
      0,
      0,
      protocol.length & 0xff,
      protocol.length >> 8,
      cookie.length & 0xff,
      cookie.length >> 8,
      0,
      0,
      ...protocol,
    ];
    while (packet.length % 4 != 0) {
      packet.add(0);
    }
    packet.addAll(cookie);
    while (packet.length % 4 != 0) {
      packet.add(0);
    }
    return packet;
  }

  static String? rejectionReason(List<int> prefix) {
    if (prefix.length < 8) return 'The X server returned a truncated reply.';
    if (prefix.first == 1) return null;
    final reasonLength = prefix[1];
    return reasonLength == 0
        ? 'The X server rejected the authenticated connection.'
        : 'The X server rejected the authenticated connection '
              '(reason length $reasonLength).';
  }

  static List<int> _decodeHexadecimalCookie(String value) {
    if (value.isEmpty || value.length.isOdd) {
      throw const FormatException('The X11 cookie has an invalid length.');
    }
    return [
      for (var index = 0; index < value.length; index += 2)
        int.parse(value.substring(index, index + 2), radix: 16),
    ];
  }
}

/// Owns one isolated VcXsrv instance for all X11 tabs in this application.
/// Access control remains enabled and every launch gets a new Xauthority
/// cookie; AetherDesk never uses VcXsrv's insecure `-ac` option.
class X11ServerManager {
  X11ServerManager({Directory? applicationDirectory})
    : _applicationDirectory =
          applicationDirectory ?? File(Platform.resolvedExecutable).parent;

  final Directory _applicationDirectory;
  X11ServerSession? _session;
  Future<X11ServerSession>? _starting;
  Future<void>? _stopping;

  // A busy X server (especially while an application holds XGrabServer) may
  // legitimately delay new setup requests. A failed probe must never kill
  // existing SSH/X11 sessions. Authentication is checked only at startup;
  // thereafter the owned process's exit notification determines its lifetime.
  Future<bool> isRunning() async => _session?.isRunning ?? false;

  int? get exitCode => _session?.exitCode;

  Future<X11ServerSession> ensureRunning() async {
    // A reconnect must not acquire the process that a disconnect is stopping.
    await _stopping;
    final pending = _starting;
    if (pending != null) return pending;
    final existing = _session;
    if (existing != null && existing.isRunning) return existing;
    final starting = _startServer();
    _starting = starting;
    try {
      return await starting;
    } finally {
      _starting = null;
    }
  }

  Future<X11ServerSession> _startServer() async {
    await _stopCurrentSession();

    final executable = bundledExecutable(_applicationDirectory);
    final xauth = File(
      '${executable.parent.path}${Platform.pathSeparator}xauth.exe',
    );

    final displayNumber = await _findDisplayNumber();
    final runtime = await _runtimeDirectory();
    final icon = await rootBundle.load('assets/aetherdesk.ico');
    await File(
      '${runtime.path}${Platform.pathSeparator}aetherdesk.ico',
    ).writeAsBytes(
      icon.buffer.asUint8List(icon.offsetInBytes, icon.lengthInBytes),
    );
    await File(
      '${runtime.path}${Platform.pathSeparator}.XWinrc',
    ).writeAsString(preferences(runtime.path));
    final authorizationFile = File(
      '${runtime.path}${Platform.pathSeparator}xauthority-$displayNumber',
    );
    final logFile = File(
      '${runtime.path}${Platform.pathSeparator}vcxsrv-$displayNumber.log',
    );
    if (await authorizationFile.exists()) {
      await authorizationFile.delete();
    }
    final cookie = X11SetupHandshake.createCookie();
    for (final display in [
      '127.0.0.1:$displayNumber',
      'localhost:$displayNumber',
    ]) {
      final result = await Process.run(xauth.path, [
        '-f',
        authorizationFile.path,
        'add',
        display,
        '.',
        cookie,
      ], workingDirectory: executable.parent.path);
      if (result.exitCode != 0) {
        if (await authorizationFile.exists()) {
          await authorizationFile.delete();
        }
        throw X11ServerException(
          'Could not create the X11 authorization cookie: '
          '${result.stderr.toString().trim()}',
        );
      }
    }

    late final Process process;
    try {
      process = await Process.start(
        executable.path,
        [
          ':$displayNumber',
          '-multiwindow',
          '-notrayicon',
          // The startup probe can be the last client while VcXsrv's internal
          // window-manager threads are still connecting. A reset at that point
          // starts a second WM generation and can terminate the whole server.
          '-noreset',
          '-clipboard',
          // Software GLX is more compatible with legacy Motif/OpenGL clients
          // (including common EDA tools) than VcXsrv's native WGL bridge.
          '-nowgl',
          '+iglx',
          '-auth',
          authorizationFile.path,
          '-logfile',
          logFile.path,
          '-silent-dup-error',
          // The Rust SSH proxy connects over IPv4 loopback. Current VcXsrv
          // builds do not guarantee that TCP listening is enabled by default.
          '-listen',
          'tcp',
          '-nolisten',
          'inet6',
        ],
        workingDirectory: executable.parent.path,
        // VcXsrv reads HOME/.XWinrc. This changes only this child process,
        // never the user's existing X-server preferences or environment.
        environment: {
          'HOME': runtime.path,
          'XKB_CONFIG_ROOT': '${executable.parent.path}/xkbdata',
          'XLOCALEDIR': '${executable.parent.path}/locale',
        },
        mode: ProcessStartMode.normal,
      );
    } on Object {
      if (await authorizationFile.exists()) {
        await authorizationFile.delete();
      }
      rethrow;
    }
    final port = 6000 + displayNumber;
    AppLog.instance.info(
      AppLogSource.x11,
      'VcXsrv started on display :$displayNumber (port $port)',
    );
    final session = X11ServerSession(
      process: process,
      displayNumber: displayNumber,
      authorizationFile: authorizationFile,
      options: X11ConnectionOptions(
        localPort: port,
        authenticationProtocol: 'MIT-MAGIC-COOKIE-1',
        authenticationCookie: cookie,
      ),
    );
    _session = session;
    for (var attempt = 0; attempt < 40; attempt++) {
      if (!session.isRunning) break;
      if (await _authenticatedConnectionWorks(port, cookie) &&
          session.isRunning) {
        if (!await _authenticatedConnectionWorks(
          port,
          '1a' * 16,
          expectAccepted: false,
        )) {
          await _stopCurrentSession();
          throw X11ServerException(
            'AetherDesk could not verify access control for its built-in X11 '
            'server. The server was stopped for safety.',
          );
        }
        return session;
      }
      await Future<void>.delayed(const Duration(milliseconds: 200));
    }
    await _stopCurrentSession();
    throw X11ServerException(
      'VcXsrv started but did not accept connections on display '
      ':$displayNumber.',
    );
  }

  Future<void> stop() async {
    final pending = _stopping;
    if (pending != null) return pending;
    final stopping = _stopAfterStartup();
    _stopping = stopping;
    try {
      await stopping;
    } finally {
      _stopping = null;
    }
  }

  Future<void> _stopAfterStartup() async {
    // Do not leave a late-starting server behind when the app is closed while
    // xauth or the initial authenticated setup exchange is still running.
    try {
      await _starting;
    } on Object {
      // Startup performs its own cleanup before reporting a failure.
    }
    await _stopCurrentSession();
  }

  Future<void> _stopCurrentSession() async {
    final session = _session;
    _session = null;
    if (session == null) return;
    if (session.isRunning) session.process.kill();
    await session.exited.timeout(const Duration(seconds: 5));
    if (await session.authorizationFile.exists()) {
      await session.authorizationFile.delete();
    }
  }

  static String preferences(String directory) {
    if (directory.contains('"') ||
        directory.contains('\n') ||
        directory.contains('\r')) {
      throw const FormatException('Invalid X11 runtime directory.');
    }
    return 'ICONDirectory "$directory"\nDEFAULTICON "aetherdesk.ico"\n';
  }

  static File bundledExecutable(Directory applicationDirectory) {
    final root = '${applicationDirectory.path}${Platform.pathSeparator}x11';
    for (final relative in [
      'vcxsrv.exe',
      'xauth.exe',
      'xkbcomp.exe',
      'fonts/misc/fonts.dir',
      'xkbdata/rules/base',
    ]) {
      if (!File('$root/$relative').existsSync()) {
        throw X11ServerException(
          'AetherDesk\'s built-in X11 files are missing or incomplete. '
          'Reinstall AetherDesk, or extract its complete portable ZIP.',
        );
      }
    }
    // Never fall back to a system installation: that hides broken packages
    // on a developer's computer and fails when a new user opens the app.
    return File('$root${Platform.pathSeparator}vcxsrv.exe');
  }

  Future<int> _findDisplayNumber() async {
    for (var display = 10; display <= 39; display++) {
      // On Windows a loopback-only bind can succeed even when an X server
      // already owns the wildcard address. Check for a listener first, then
      // reserve the same wildcard address VcXsrv will need.
      Socket? existing;
      try {
        existing = await Socket.connect(
          InternetAddress.loopbackIPv4,
          6000 + display,
          timeout: const Duration(milliseconds: 250),
        );
        continue;
      } on SocketException {
        // No reachable listener; the bind below still checks port ownership.
      } finally {
        existing?.destroy();
      }
      try {
        final probe = await ServerSocket.bind(
          InternetAddress.anyIPv4,
          6000 + display,
          shared: false,
        );
        await probe.close();
        return display;
      } on SocketException {
        // The display is already occupied; try the next isolated display.
      }
    }
    throw X11ServerException(
      'No free local X11 display was available between :10 and :39.',
    );
  }

  Future<bool> _authenticatedConnectionWorks(
    int port,
    String cookie, {
    bool expectAccepted = true,
  }) async {
    Socket? socket;
    try {
      socket = await Socket.connect(
        InternetAddress.loopbackIPv4,
        port,
        timeout: const Duration(milliseconds: 350),
      );
      socket.add(X11SetupHandshake.packet(cookie));
      await socket.flush();
      final prefix = await socket
          .take(8)
          .expand((chunk) => chunk)
          .take(8)
          .toList()
          .timeout(const Duration(seconds: 1));
      return expectAccepted
          ? X11SetupHandshake.rejectionReason(prefix) == null
          : prefix.length >= 8 && prefix.first == 0;
    } on Object {
      return false;
    } finally {
      socket?.destroy();
    }
  }

  Future<Directory> _runtimeDirectory() async {
    final localAppData = Platform.environment['LOCALAPPDATA'];
    final base = localAppData == null || localAppData.isEmpty
        ? Directory.systemTemp
        : Directory(localAppData);
    final runtime = Directory(
      '${base.path}${Platform.pathSeparator}AetherDesk'
      '${Platform.pathSeparator}runtime${Platform.pathSeparator}x11',
    );
    await runtime.create(recursive: true);
    return runtime;
  }
}
