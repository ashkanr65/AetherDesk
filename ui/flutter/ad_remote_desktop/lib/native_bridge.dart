import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'app_log.dart';

import 'package:ffi/ffi.dart';

typedef _ConnectNative =
    Pointer<Utf8> Function(Pointer<Utf8>, Uint16, Pointer<Utf8>, Pointer<Utf8>);
typedef _ConnectDart =
    Pointer<Utf8> Function(Pointer<Utf8>, int, Pointer<Utf8>, Pointer<Utf8>);
typedef _ConnectEx3Native =
    Pointer<Utf8> Function(
      Pointer<Utf8>,
      Uint16,
      Pointer<Utf8>,
      Pointer<Utf8>,
      Pointer<Utf8>,
      Pointer<Utf8>,
      Int32,
      Int32,
      Uint16,
      Pointer<Utf8>,
      Pointer<Utf8>,
      Uint32,
      Pointer<Utf8>,
    );
typedef _ConnectEx3Dart =
    Pointer<Utf8> Function(
      Pointer<Utf8>,
      int,
      Pointer<Utf8>,
      Pointer<Utf8>,
      Pointer<Utf8>,
      Pointer<Utf8>,
      int,
      int,
      int,
      Pointer<Utf8>,
      Pointer<Utf8>,
      int,
      Pointer<Utf8>,
    );
typedef _TwoStringNative = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _TwoStringDart = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _WriteNative = Int32 Function(Pointer<Utf8>, Pointer<Utf8>);
typedef _WriteDart = int Function(Pointer<Utf8>, Pointer<Utf8>);

typedef _BytesNative = Int32 Function(Pointer<Utf8>, Pointer<Uint8>, UintPtr);
typedef _BytesDart = int Function(Pointer<Utf8>, Pointer<Uint8>, int);
typedef _LogInitNative = Void Function(Pointer<Utf8>);
typedef _LogInitDart = void Function(Pointer<Utf8>);
typedef _OneStringNative = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _OneStringDart = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _NoArgStringNative = Pointer<Utf8> Function();
typedef _NoArgStringDart = Pointer<Utf8> Function();
typedef _DisconnectNative = Int32 Function(Pointer<Utf8>);
typedef _DisconnectDart = int Function(Pointer<Utf8>);
typedef _NoArgIntNative = Int32 Function();
typedef _NoArgIntDart = int Function();
typedef _ResizeNative = Int32 Function(Pointer<Utf8>, Uint16, Uint16);
typedef _ResizeDart = int Function(Pointer<Utf8>, int, int);
typedef _StartForwardNative =
    Uint16 Function(Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>, Uint16);
typedef _StartForwardDart =
    int Function(Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>, int);
typedef _TransferNative =
    Int32 Function(Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>);
typedef _TransferDart =
    int Function(Pointer<Utf8>, Pointer<Utf8>, Pointer<Utf8>);
typedef _FreeNative = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

class SftpEntry {
  const SftpEntry({
    required this.name,
    required this.path,
    required this.kind,
    this.size,
    this.permissions,
    this.modified,
  });

  final String name;
  final String path;
  final String kind;
  final int? size;
  final int? permissions;
  final int? modified;

  bool get isDirectory => kind == 'directory';

  factory SftpEntry.fromJson(Map<String, Object?> json) => SftpEntry(
    name: json['name'] as String,
    path: json['path'] as String,
    kind: json['kind'] as String? ?? 'file',
    size: (json['size'] as num?)?.toInt(),
    permissions: (json['permissions'] as num?)?.toInt(),
    modified: (json['modified'] as num?)?.toInt(),
  );
}

class CoreCapabilities {
  const CoreCapabilities({
    required this.ssh,
    required this.sftp,
    required this.password,
    required this.sshKey,
    required this.ed25519,
    required this.remoteDesktop,
    required this.sshX11,
    required this.bundledX11,
    required this.smartCard,
    required this.mfa,
    required this.fido2,
  });

  final bool ssh;
  final bool sftp;
  final bool password;
  final bool sshKey;
  final bool ed25519;

  /// True only when the native in-process renderer is implemented.
  final bool remoteDesktop;
  final bool sshX11;
  final bool bundledX11;
  final bool smartCard;
  final bool mfa;
  final bool fido2;

  /// External compatibility launchers are deliberately excluded. Remote
  /// Desktop is available only when an in-process renderer is linked.
  bool get supportsRemoteDesktop => remoteDesktop;

  factory CoreCapabilities.fromJson(Map<String, Object?> json) =>
      CoreCapabilities(
        ssh: json['ssh'] as bool? ?? false,
        sftp: json['sftp'] as bool? ?? false,
        password: json['password'] as bool? ?? false,
        sshKey: json['sshKey'] as bool? ?? false,
        ed25519: json['ed25519'] as bool? ?? false,
        remoteDesktop: json['remoteDesktop'] as bool? ?? false,
        sshX11: json['sshX11'] as bool? ?? false,
        bundledX11: json['bundledX11'] as bool? ?? false,
        smartCard: json['smartCard'] as bool? ?? false,
        mfa: json['mfa'] as bool? ?? false,
        fido2: json['fido2'] as bool? ?? false,
      );
}

class X11ConnectionOptions {
  const X11ConnectionOptions({
    required this.localPort,
    required this.authenticationProtocol,
    required this.authenticationCookie,
    this.screenNumber = 0,
  });

  final int localPort;
  final String authenticationProtocol;
  final String authenticationCookie;
  final int screenNumber;
}

class NativeBridge {
  NativeBridge() : _library = _openLibrary() {
    _free = _library.lookupFunction<_FreeNative, _FreeDart>('ad_free_string');
    _initLogging();
  }

  static bool _loggingInitialized = false;

  /// Supplies the directory for the Rust core's independent log file. The
  /// call is idempotent and skipped for stale DLLs (log_feature.md phase 2).
  void _initLogging() {
    if (_loggingInitialized || !_library.providesSymbol('ad_log_init')) return;
    final function = _library.lookupFunction<_LogInitNative, _LogInitDart>(
      'ad_log_init',
    );
    final pathPointer = AppLog.instance.path.toNativeUtf8();
    try {
      function(pathPointer);
      _loggingInitialized = true;
    } finally {
      calloc.free(pathPointer);
    }
  }

  final DynamicLibrary _library;
  late final _FreeDart _free;
  final _terminalText = StringBuffer();
  late final ByteConversionSink _terminalDecoder = const Utf8Decoder(
    allowMalformed: true,
  ).startChunkedConversion(StringConversionSink.fromStringSink(_terminalText));

  String _startRemoteSync(String sshId, Map<String, Object?> config) {
    final function = _library.lookupFunction<_TwoStringNative, _TwoStringDart>(
      'ad_remote_start',
    );
    final id = sshId.toNativeUtf8();
    final json = jsonEncode(config).toNativeUtf8();
    try {
      return _consume(function(id, json));
    } finally {
      calloc.free(id);
      calloc.free(json);
    }
  }

  static Future<String> startRemote(
    String sshId,
    Map<String, Object?> config,
  ) => Isolate.run(() => NativeBridge()._startRemoteSync(sshId, config));

  void remoteCommand(String sessionId, Map<String, Object?> command) {
    final function = _library.lookupFunction<_WriteNative, _WriteDart>(
      'ad_remote_command',
    );
    final id = sessionId.toNativeUtf8();
    final json = jsonEncode(command).toNativeUtf8();
    try {
      if (function(id, json) != 0) throw NativeException(lastError());
    } finally {
      calloc.free(id);
      calloc.free(json);
    }
  }

  Map<String, Object?> remoteStatus(String sessionId) {
    final function = _library.lookupFunction<_OneStringNative, _OneStringDart>(
      'ad_remote_status',
    );
    final id = sessionId.toNativeUtf8();
    try {
      return Map<String, Object?>.from(
        jsonDecode(_consume(function(id))) as Map,
      );
    } finally {
      calloc.free(id);
    }
  }

  void _closeRemoteSync(String sessionId, bool terminate) {
    final function = _library
        .lookupFunction<_DisconnectNative, _DisconnectDart>(
          terminate ? 'ad_remote_end' : 'ad_remote_close',
        );
    final id = sessionId.toNativeUtf8();
    try {
      if (function(id) != 0) throw NativeException(lastError());
    } finally {
      calloc.free(id);
    }
  }

  static Future<void> closeRemote(String sessionId, {bool terminate = false}) =>
      Isolate.run(() => NativeBridge()._closeRemoteSync(sessionId, terminate));

  static DynamicLibrary _openLibrary() {
    if (!Platform.isWindows) return DynamicLibrary.process();
    final executableDirectory = File(Platform.resolvedExecutable).parent.path;
    final packagedLibrary = File(
      '$executableDirectory${Platform.pathSeparator}ad_ffi.dll',
    );
    if (packagedLibrary.existsSync()) {
      return DynamicLibrary.open(packagedLibrary.path);
    }
    return DynamicLibrary.open('ad_ffi.dll');
  }

  static String? availabilityError() {
    try {
      NativeBridge().versionSync();
      return null;
    } on Object catch (error) {
      return error.toString();
    }
  }

  String _consume(Pointer<Utf8> pointer, {bool allowNull = false}) {
    if (pointer == nullptr) {
      if (allowNull) return '';
      throw NativeException(lastError());
    }
    try {
      return pointer.toDartString();
    } finally {
      _free(pointer);
    }
  }

  String lastError() {
    final function = _library
        .lookupFunction<_NoArgStringNative, _NoArgStringDart>('ad_last_error');
    final pointer = function();
    return _consume(pointer, allowNull: true);
  }

  String versionSync() {
    final function = _library
        .lookupFunction<_NoArgStringNative, _NoArgStringDart>('ad_get_version');
    return _consume(function());
  }

  CoreCapabilities capabilitiesSync() {
    final function = _library
        .lookupFunction<_NoArgStringNative, _NoArgStringDart>(
          'ad_get_capabilities',
        );
    return CoreCapabilities.fromJson(
      Map<String, Object?>.from(jsonDecode(_consume(function())) as Map),
    );
  }

  String connectSync(String host, int port, String username, String password) {
    final function = _library.lookupFunction<_ConnectNative, _ConnectDart>(
      'ad_ssh_connect',
    );
    final hostPointer = host.toNativeUtf8();
    final userPointer = username.toNativeUtf8();
    final passwordPointer = password.toNativeUtf8();
    try {
      return _consume(
        function(hostPointer, port, userPointer, passwordPointer),
      );
    } finally {
      calloc.free(hostPointer);
      calloc.free(userPointer);
      calloc.free(passwordPointer);
    }
  }

  static Future<String> connect(
    String host,
    int port,
    String username,
    String secret, {
    String authentication = 'password',
    String keyPath = '',
    String wireguardConfigPath = '',
    bool trustNewHostKey = false,
    bool openShell = true,
    X11ConnectionOptions? x11,
  }) async {
    try {
      return await Isolate.run(
        () => NativeBridge().connectExSync(
          host,
          port,
          username,
          secret,
          authentication: authentication,
          keyPath: keyPath,
          wireguardConfigPath: wireguardConfigPath,
          trustNewHostKey: trustNewHostKey,
          openShell: openShell,
          x11: x11,
        ),
      );
    } on Object catch (error) {
      final hostKey = UnknownHostKeyException.tryParse(error.toString());
      if (hostKey != null) throw hostKey;
      rethrow;
    }
  }

  String connectExSync(
    String host,
    int port,
    String username,
    String secret, {
    required String authentication,
    required String keyPath,
    String wireguardConfigPath = '',
    required bool trustNewHostKey,
    required bool openShell,
    X11ConnectionOptions? x11,
  }) {
    final function = _library
        .lookupFunction<_ConnectEx3Native, _ConnectEx3Dart>(
          'ad_ssh_connect_ex3',
        );
    final hostPointer = host.toNativeUtf8();
    final userPointer = username.toNativeUtf8();
    final secretPointer = secret.toNativeUtf8();
    final authenticationPointer = authentication.toNativeUtf8();
    final keyPathPointer = keyPath.toNativeUtf8();
    final vpnPathPointer = wireguardConfigPath.toNativeUtf8();
    final x11ProtocolPointer = (x11?.authenticationProtocol ?? '')
        .toNativeUtf8();
    final x11CookiePointer = (x11?.authenticationCookie ?? '').toNativeUtf8();
    try {
      return _consume(
        function(
          hostPointer,
          port,
          userPointer,
          secretPointer,
          authenticationPointer,
          keyPathPointer,
          trustNewHostKey ? 1 : 0,
          openShell ? 1 : 0,
          x11?.localPort ?? 0,
          x11ProtocolPointer,
          x11CookiePointer,
          x11?.screenNumber ?? 0,
          vpnPathPointer,
        ),
      );
    } finally {
      calloc.free(hostPointer);
      calloc.free(userPointer);
      calloc.free(secretPointer);
      calloc.free(authenticationPointer);
      calloc.free(keyPathPointer);
      calloc.free(vpnPathPointer);
      calloc.free(x11ProtocolPointer);
      calloc.free(x11CookiePointer);
    }
  }

  int startLocalForwardSync(
    String sessionId,
    String localHost,
    String remoteHost,
    int remotePort,
  ) {
    final function = _library
        .lookupFunction<_StartForwardNative, _StartForwardDart>(
          'ad_ssh_start_local_forward',
        );
    final sessionPointer = sessionId.toNativeUtf8();
    final localHostPointer = localHost.toNativeUtf8();
    final hostPointer = remoteHost.toNativeUtf8();
    try {
      final localPort = function(
        sessionPointer,
        localHostPointer,
        hostPointer,
        remotePort,
      );
      if (localPort == 0) throw NativeException(lastError());
      return localPort;
    } finally {
      calloc.free(sessionPointer);
      calloc.free(localHostPointer);
      calloc.free(hostPointer);
    }
  }

  static Future<int> startLocalForward(
    String sessionId,
    String localHost,
    String remoteHost,
    int remotePort,
  ) => Isolate.run(
    () => NativeBridge().startLocalForwardSync(
      sessionId,
      localHost,
      remoteHost,
      remotePort,
    ),
  );

  String executeCommandSync(String sessionId, String command) {
    final function = _library.lookupFunction<_TwoStringNative, _TwoStringDart>(
      'ad_ssh_execute_command',
    );
    final sessionPointer = sessionId.toNativeUtf8();
    final commandPointer = command.toNativeUtf8();
    try {
      return _consume(function(sessionPointer, commandPointer));
    } finally {
      calloc.free(sessionPointer);
      calloc.free(commandPointer);
    }
  }

  static Future<String> executeCommand(String sessionId, String command) =>
      Isolate.run(() => NativeBridge().executeCommandSync(sessionId, command));

  void write(String sessionId, String data) {
    final function = _library.lookupFunction<_BytesNative, _BytesDart>(
      'ad_ssh_write_bytes',
    );
    final idPointer = sessionId.toNativeUtf8();
    final bytes = utf8.encode(data);
    final dataPointer = calloc<Uint8>(bytes.isEmpty ? 1 : bytes.length);
    dataPointer.asTypedList(bytes.length).setAll(0, bytes);
    try {
      if (function(idPointer, dataPointer, bytes.length) != 0) {
        throw NativeException(lastError());
      }
    } finally {
      calloc.free(idPointer);
      calloc.free(dataPointer);
    }
  }

  // One buffer for the terminal poll (increase_speed.md D1): a 64 KB
  // calloc/free pair per tick per tab dominated the UI thread. The decoder
  // consumes the view synchronously in add(), so reuse is safe.
  static final Pointer<Uint8> _readBuffer = calloc<Uint8>(65536);
  static _BytesDart? _readFunction;

  String read(String sessionId) {
    final function = _readFunction ??= _library
        .lookupFunction<_BytesNative, _BytesDart>('ad_ssh_read_bytes');
    final idPointer = sessionId.toNativeUtf8();
    try {
      final length = function(idPointer, _readBuffer, 65536);
      if (length < 0) throw NativeException(lastError());
      _terminalDecoder.add(_readBuffer.asTypedList(length));
      final text = _terminalText.toString();
      _terminalText.clear();
      return text;
    } finally {
      calloc.free(idPointer);
    }
  }

  void resize(String sessionId, int columns, int rows) {
    final function = _library.lookupFunction<_ResizeNative, _ResizeDart>(
      'ad_ssh_resize',
    );
    final idPointer = sessionId.toNativeUtf8();
    try {
      if (function(idPointer, columns, rows) != 0) {
        throw NativeException(lastError());
      }
    } finally {
      calloc.free(idPointer);
    }
  }

  void disconnectSync(String sessionId) {
    final function = _library
        .lookupFunction<_DisconnectNative, _DisconnectDart>(
          'ad_ssh_disconnect',
        );
    final idPointer = sessionId.toNativeUtf8();
    try {
      if (function(idPointer) != 0) throw NativeException(lastError());
    } finally {
      calloc.free(idPointer);
    }
  }

  static Future<void> disconnect(String sessionId) =>
      Isolate.run(() => NativeBridge().disconnectSync(sessionId));

  /// Close every remaining SSH connection and detach remote desktops. Runs
  /// synchronously on purpose: application exit must not race a worker isolate.
  static Future<void> shutdownAll() =>
      Isolate.run(() => NativeBridge().shutdownAllSync());

  void shutdownAllSync() {
    _library.lookupFunction<_NoArgIntNative, _NoArgIntDart>(
      'ad_ssh_shutdown_all',
    )();
    _library.lookupFunction<_NoArgIntNative, _NoArgIntDart>(
      'ad_remote_close_all',
    )();
  }

  Map<String, dynamic> _fileCall(
    String symbol,
    String value, [
    String? request,
  ]) {
    final pointer = value.toNativeUtf8();
    final requestPointer = request?.toNativeUtf8();
    try {
      final result = requestPointer == null
          ? _library.lookupFunction<_OneStringNative, _OneStringDart>(symbol)(
              pointer,
            )
          : _library.lookupFunction<_TwoStringNative, _TwoStringDart>(symbol)(
              pointer,
              requestPointer,
            );
      return Map<String, dynamic>.from(jsonDecode(_consume(result)) as Map);
    } finally {
      calloc.free(pointer);
      if (requestPointer != null) calloc.free(requestPointer);
    }
  }

  static Future<String> startFileJob(
    String session,
    Map<String, Object?> request,
  ) => Isolate.run(
    () =>
        NativeBridge()._fileCall(
              'ad_file_start',
              session,
              jsonEncode(request),
            )['id']
            as String,
  );
  static Future<Map<String, dynamic>> pollFileJob(String id) =>
      Isolate.run(() => NativeBridge()._fileCall('ad_file_poll', id));
  static Future<void> cancelFileJob(String id) => Isolate.run(() {
    final bridge = NativeBridge();
    final pointer = id.toNativeUtf8();
    try {
      if (bridge._library.lookupFunction<_DisconnectNative, _DisconnectDart>(
            'ad_file_cancel',
          )(pointer) !=
          0)
        throw NativeException(bridge.lastError());
    } finally {
      calloc.free(pointer);
    }
  });

  static Future<String> startLanguageServer(
    String session,
    Map<String, Object?> config,
  ) => Isolate.run(
    () =>
        NativeBridge()._fileCall(
              'ad_lsp_start',
              session,
              jsonEncode(config),
            )['id']
            as String,
  );
  static Future<Map<String, dynamic>> pollLanguageServer(String id) =>
      Isolate.run(() => NativeBridge()._fileCall('ad_lsp_poll', id));
  static Future<void> sendLanguageServer(
    String id,
    Map<String, Object?> message,
  ) => Isolate.run(() {
    final bridge = NativeBridge();
    final pointer = id.toNativeUtf8();
    final content = jsonEncode(message).toNativeUtf8();
    try {
      if (bridge._library.lookupFunction<_WriteNative, _WriteDart>(
            'ad_lsp_send',
          )(pointer, content) !=
          0)
        throw NativeException(bridge.lastError());
    } finally {
      calloc.free(pointer);
      calloc.free(content);
    }
  });
  static Future<void> stopLanguageServer(String id) => Isolate.run(() {
    final bridge = NativeBridge();
    final pointer = id.toNativeUtf8();
    try {
      if (bridge._library.lookupFunction<_DisconnectNative, _DisconnectDart>(
            'ad_lsp_stop',
          )(pointer) !=
          0)
        throw NativeException(bridge.lastError());
    } finally {
      calloc.free(pointer);
    }
  });

  List<SftpEntry> listSync(String sessionId, String remotePath) {
    final function = _library.lookupFunction<_TwoStringNative, _TwoStringDart>(
      'ad_sftp_list',
    );
    final idPointer = sessionId.toNativeUtf8();
    final pathPointer = remotePath.toNativeUtf8();
    try {
      final decoded =
          jsonDecode(_consume(function(idPointer, pathPointer)))
              as List<dynamic>;
      return decoded
          .map(
            (item) =>
                SftpEntry.fromJson(Map<String, Object?>.from(item as Map)),
          )
          .toList();
    } finally {
      calloc.free(idPointer);
      calloc.free(pathPointer);
    }
  }

  static Future<List<SftpEntry>> list(String sessionId, String remotePath) =>
      Isolate.run(() => NativeBridge().listSync(sessionId, remotePath));

  void _transferSync(
    String symbol,
    String sessionId,
    String firstPath,
    String secondPath,
  ) {
    final function = _library.lookupFunction<_TransferNative, _TransferDart>(
      symbol,
    );
    final idPointer = sessionId.toNativeUtf8();
    final firstPointer = firstPath.toNativeUtf8();
    final secondPointer = secondPath.toNativeUtf8();
    try {
      if (function(idPointer, firstPointer, secondPointer) != 0) {
        throw NativeException(lastError());
      }
    } finally {
      calloc.free(idPointer);
      calloc.free(firstPointer);
      calloc.free(secondPointer);
    }
  }

  static Future<void> upload(
    String sessionId,
    String localPath,
    String remotePath, {
    bool resume = false,
  }) => Isolate.run(
    () => NativeBridge()._transferSync(
      resume ? 'ad_sftp_upload_file_resume' : 'ad_sftp_upload_file',
      sessionId,
      localPath,
      remotePath,
    ),
  );

  static Future<void> download(
    String sessionId,
    String remotePath,
    String localPath, {
    bool resume = false,
  }) => Isolate.run(
    () => NativeBridge()._transferSync(
      resume ? 'ad_sftp_download_file_resume' : 'ad_sftp_download_file',
      sessionId,
      remotePath,
      localPath,
    ),
  );
}

class NativeException implements Exception {
  NativeException(this.message) {
    // Every native failure funnels through this constructor: persist the
    // message with its call site so it survives on screen (log_feature.md B).
    final caller = StackTrace.current
        .toString()
        .split('\n')
        .take(2)
        .map((line) => line.trim())
        .join(' <- ');
    AppLog.instance.error(AppLogSource.coreFfi, message, target: caller);
  }

  final String message;

  @override
  String toString() => message.isEmpty ? 'Native operation failed' : message;
}

class UnknownHostKeyException implements Exception {
  const UnknownHostKeyException({
    required this.host,
    required this.port,
    required this.algorithm,
    required this.fingerprint,
  });

  final String host;
  final int port;
  final String algorithm;
  final String fingerprint;

  static UnknownHostKeyException? tryParse(String message) {
    const marker = 'AETHERDESK_HOST_KEY_UNKNOWN|';
    final start = message.indexOf(marker);
    if (start < 0) return null;
    final fields = message.substring(start + marker.length).split('|');
    if (fields.length < 4) return null;
    return UnknownHostKeyException(
      host: fields[0],
      port: int.tryParse(fields[1]) ?? 22,
      algorithm: fields[2],
      fingerprint: fields[3].split(RegExp(r'[\r\n ]'))[0],
    );
  }

  @override
  String toString() => 'Untrusted host key: $fingerprint';
}
