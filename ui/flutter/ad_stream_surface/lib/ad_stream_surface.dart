import 'dart:async';
import 'dart:ui' as ui;
import 'package:flutter/services.dart';

/// Owns native texture registration; session traffic stays in the Rust core.
class AdStreamSurface {
  static const _channel = MethodChannel('aetherdesk/stream_surface');

  static Future<int> create(String sessionId) async {
    final id = await _channel.invokeMethod<int>('create', sessionId);
    if (id == null || id < 0)
      throw StateError('Native texture registration failed');
    return id;
  }

  static Future<Map<String, String>?> keyboardLayout() async {
    final result = await _channel.invokeMapMethod<String, String>(
      'keyboardLayout',
    );
    return result;
  }

  static Future<bool> isPresented(int textureId) async =>
      await _channel.invokeMethod<bool>('isPresented', textureId) ?? false;

  static Future<void> present(int textureId, {required bool allMonitors}) =>
      _channel.invokeMethod<void>('present', {
        'textureId': textureId,
        'allMonitors': allMonitors,
      });

  /// Update the physical tab area. Native presentations retain this as their
  /// return target without changing the active fullscreen monitor layout.
  static Future<void> setTabSize(
    int textureId,
    int width,
    int height, {
    bool force = false,
  }) => _channel.invokeMethod<void>('setTabSize', {
    'textureId': textureId,
    'width': width,
    'height': height,
    'force': force,
  });

  static Future<ui.Image> capture(int textureId) async {
    final frame = await _channel.invokeMapMethod<String, dynamic>(
      'capture',
      textureId,
    );
    if (frame == null) throw StateError('The remote screen is unavailable.');
    final image = Completer<ui.Image>();
    ui.decodeImageFromPixels(
      frame['rgba'] as Uint8List,
      frame['width'] as int,
      frame['height'] as int,
      ui.PixelFormat.rgba8888,
      image.complete,
    );
    return image.future;
  }

  static Future<void> dispose(int textureId) =>
      _channel.invokeMethod<void>('dispose', textureId);
}
