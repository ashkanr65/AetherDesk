import 'dart:async';
import 'dart:io';
import 'dart:convert';
import 'package:crypto/crypto.dart';

import 'package:ad_stream_surface/ad_stream_surface.dart';
import 'package:flutter/foundation.dart';

import 'native_bridge.dart';
import 'app_version.dart';
import 'session_store.dart';

class RemoteDesktopException implements Exception {
  const RemoteDesktopException(this.message);
  final String message;
  @override
  String toString() => message;
}

enum RemoteDesktopBackend { aetherStream, unavailable }

class RemoteDesktopReadiness {
  const RemoteDesktopReadiness({
    required this.linuxReady,
    required this.desktopReady,
    required this.dbusReady,
    this.missingDesktopCommand,
  });
  final bool linuxReady;
  final bool desktopReady;
  final bool dbusReady;
  final String? missingDesktopCommand;
  RemoteDesktopBackend get selectedBackend =>
      linuxReady && dbusReady && desktopReady
      ? RemoteDesktopBackend.aetherStream
      : RemoteDesktopBackend.unavailable;
  String get diagnosticSummary =>
      'backend=${selectedBackend.name};linux=$linuxReady;'
      'desktop=$desktopReady;dbus=$dbusReady;missingDesktop=${missingDesktopCommand ?? ''}';
  factory RemoteDesktopReadiness.parse(String output) {
    final lines = output
        .split(RegExp(r'[\r\n]+'))
        .map((line) => line.trim())
        .toSet();
    const marker = 'AETHERDESK_DESKTOP_MISSING:';
    final missing = lines.where((line) => line.startsWith(marker)).firstOrNull;
    return RemoteDesktopReadiness(
      linuxReady: lines.contains('AETHERDESK_LINUX_X64_READY'),
      desktopReady: lines.contains('AETHERDESK_DESKTOP_READY'),
      dbusReady: lines.contains('AETHERDESK_DBUS_READY'),
      missingDesktopCommand: missing?.substring(marker.length),
    );
  }
}

class RemoteDesktopStatus {
  const RemoteDesktopStatus({
    this.width = 1280,
    this.height = 720,
    this.frames = 0,
    this.error,
  });
  final int width;
  final int height;
  final int frames;
  final String? error;

  // The 250 ms status poll must not rebuild the shell while nothing changed
  // (increase_speed.md D2).
  @override
  bool operator ==(Object other) =>
      other is RemoteDesktopStatus &&
      other.width == width &&
      other.height == height &&
      other.frames == frames &&
      other.error == error;

  @override
  int get hashCode => Object.hash(width, height, frames, error);
}

abstract interface class RemoteDesktopSession {
  int get textureId;
  ValueListenable<RemoteDesktopStatus> get status;
  Future<void> get closed;
  void sendInput(Map<String, Object?> event);
  void resize(int width, int height);
  void publishClipboard(String text);
  String? takeClipboard();
  Future<void> dispose({bool terminate = false});
  void setKeyboardLayout(String layout, String variant);
}

/// Stable, non-secret desktop identity scoped to profile, host and SSH account.
/// UUIDv8 contains a truncated SHA-256 digest; changing a profile name keeps it.
String persistentDesktopId(SavedSession profile, String username) {
  final bytes = sha256
      .convert(
        utf8.encode(
          jsonEncode([
            'AetherDesk desktop v1',
            profile.id,
            profile.host.toLowerCase(),
            profile.port,
            username,
            profile.desktopEnvironment.name,
          ]),
        ),
      )
      .bytes
      .take(16)
      .toList();
  bytes[6] = (bytes[6] & 0x0f) | 0x80;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  final hex = bytes.map((v) => v.toRadixString(16).padLeft(2, '0')).join();
  return '${hex.substring(0, 8)}-${hex.substring(8, 12)}-${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}';
}

class RemoteDesktopLauncher {
  const RemoteDesktopLauncher();

  static String readinessCommand(DesktopEnvironment environment) {
    final desktop = switch (environment) {
      DesktopEnvironment.xfce => 'startxfce4',
      DesktopEnvironment.kde => 'startplasma-x11',
      DesktopEnvironment.gnome => 'gnome-session',
      DesktopEnvironment.mate => 'mate-session',
      DesktopEnvironment.cinnamon => 'cinnamon-session',
      DesktopEnvironment.lxqt => 'startlxqt',
    };
    return 'if [ "\$(uname -s)" = Linux ] && [ "\$(uname -m)" = x86_64 ]; then '
        "printf 'AETHERDESK_LINUX_X64_READY\\n'; fi; "
        "if command -v dbus-run-session >/dev/null 2>&1; then printf 'AETHERDESK_DBUS_READY\\n'; fi; "
        "if command -v $desktop >/dev/null 2>&1; then printf 'AETHERDESK_DESKTOP_READY\\n'; "
        "else printf 'AETHERDESK_DESKTOP_MISSING:$desktop\\n'; fi";
  }

  Future<RemoteDesktopSession> launch({
    required String sshSessionId,
    required SavedSession profile,
    required String username,
    required String sshSecret,
  }) async {
    final base = File(Platform.resolvedExecutable).parent.path;
    final runtime =
        '$base${Platform.pathSeparator}agent${Platform.pathSeparator}linux-x86_64';
    final agent = File('$runtime${Platform.pathSeparator}ad-agent');
    if (!agent.existsSync() ||
        !File(
          '$runtime${Platform.pathSeparator}runtime-manifest.json',
        ).existsSync()) {
      throw const RemoteDesktopException(
        'This package is missing the Linux desktop agent. Use the complete AetherDesk Windows download.',
      );
    }
    final sessionId = await NativeBridge.startRemote(sshSessionId, {
      'desktopSessionId': persistentDesktopId(profile, username),
      'agentPath': agent.path,
      'runtimeDirectory': runtime,
      'agentVersion': agentVersion,
      'desktopEnvironment': profile.desktopEnvironment.name,
      'width': 1280,
      'height': 720,
    });
    try {
      final textureId = await AdStreamSurface.create(sessionId);
      return _NativeRemoteDesktopSession(sessionId, textureId);
    } on Object {
      await NativeBridge.closeRemote(sessionId);
      rethrow;
    }
  }
}

class _NativeRemoteDesktopSession implements RemoteDesktopSession {
  _NativeRemoteDesktopSession(this.id, this.textureId) {
    _timer = Timer.periodic(const Duration(milliseconds: 250), (_) => _poll());
  }
  final String id;
  @override
  final int textureId;
  final NativeBridge _native = NativeBridge();
  final ValueNotifier<RemoteDesktopStatus> _status = ValueNotifier(
    const RemoteDesktopStatus(),
  );
  final Completer<void> _closed = Completer<void>();
  late final Timer _timer;
  Future<void>? _disposing;
  bool _released = false;
  String? _clipboard;
  @override
  ValueListenable<RemoteDesktopStatus> get status => _status;
  @override
  Future<void> get closed => _closed.future;

  void _poll() {
    if (_disposing != null) return;
    try {
      final state = _native.remoteStatus(id);
      _status.value = RemoteDesktopStatus(
        width: (state['width'] as num).toInt(),
        height: (state['height'] as num).toInt(),
        frames: (state['frames'] as num).toInt(),
        error: state['error'] as String?,
      );
      _clipboard = state['clipboard'] as String? ?? _clipboard;
      if (state['state'] != 'connected') {
        _timer.cancel();
        if (!_closed.isCompleted) _closed.complete();
      }
    } on Object catch (error) {
      _timer.cancel();
      _status.value = RemoteDesktopStatus(error: error.toString());
      if (!_closed.isCompleted) _closed.complete();
    }
  }

  @override
  void sendInput(Map<String, Object?> event) {
    if (_disposing == null)
      _native.remoteCommand(id, {'type': 'input', 'event': event});
  }

  @override
  void resize(int width, int height) {
    if (_disposing == null)
      _native.remoteCommand(id, {
        'type': 'resize',
        'width': width,
        'height': height,
      });
  }

  @override
  void publishClipboard(String text) {
    if (_disposing == null)
      _native.remoteCommand(id, {'type': 'clipboard', 'text': text});
  }

  @override
  String? takeClipboard() {
    final text = _clipboard;
    _clipboard = null;
    return text;
  }

  @override
  void setKeyboardLayout(String layout, String variant) {
    if (_disposing == null)
      _native.remoteCommand(id, {
        'type': 'keyboardLayout',
        'layout': layout,
        'variant': variant,
      });
  }

  @override
  Future<void> dispose({bool terminate = false}) =>
      _released ? Future.value() : (_disposing ??= _dispose(terminate));
  Future<void> _dispose(bool terminate) async {
    _timer.cancel();
    try {
      // Release local windows, hooks and frame access before waiting on the host.
      try {
        await AdStreamSurface.dispose(textureId);
      } on Object {
        // A local presentation cleanup failure says nothing about the host.
        // Always ask the core to end/detach and use its authoritative result.
        debugPrint('Remote presentation cleanup did not finish.');
      }
      await NativeBridge.closeRemote(id, terminate: terminate);
      final previous = _status.value;
      _status.value = RemoteDesktopStatus(
        width: previous.width,
        height: previous.height,
        frames: previous.frames,
      );
    } on Object catch (error) {
      final previous = _status.value;
      _status.value = RemoteDesktopStatus(
        width: previous.width,
        height: previous.height,
        frames: previous.frames,
        error: error.toString(),
      );
      rethrow;
    } finally {
      _released = true;
      if (!_closed.isCompleted) _closed.complete();
    }
  }
}
