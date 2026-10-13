import 'dart:convert';
import 'dart:ui' as ui;
import 'package:flutter/painting.dart';

/// Takes ownership of the image and bounds both dimensions and API payload.
Future<Map<String, dynamic>> encodeConnectionScreen(
  ui.Image image,
  String label,
) async {
  try {
    for (var attempt = 0; attempt < 8; attempt++) {
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      if (bytes == null)
        throw StateError('Could not encode the connection screen.');
      if (bytes.lengthInBytes <= 1024 * 1024) {
        return {
          'label': label,
          'url':
              'data:image/png;base64,${base64Encode(bytes.buffer.asUint8List(bytes.offsetInBytes, bytes.lengthInBytes))}',
        };
      }
      final recorder = ui.PictureRecorder();
      final canvas = Canvas(recorder);
      final width = (image.width * .7).round().clamp(1, 1600);
      final height = (image.height * .7).round().clamp(1, 1600);
      canvas.drawImageRect(
        image,
        Rect.fromLTWH(0, 0, image.width.toDouble(), image.height.toDouble()),
        Rect.fromLTWH(0, 0, width.toDouble(), height.toDouble()),
        Paint()..filterQuality = FilterQuality.medium,
      );
      final picture = recorder.endRecording();
      final scaled = await picture.toImage(width, height);
      picture.dispose();
      image.dispose();
      image = scaled;
    }
    throw StateError('The connection screen is too large.');
  } finally {
    image.dispose();
  }
}
