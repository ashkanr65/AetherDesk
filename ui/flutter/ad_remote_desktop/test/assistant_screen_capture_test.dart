import 'dart:convert';
import 'dart:ui' as ui;
import 'package:ad_stream_surface/ad_stream_surface.dart';
import 'package:ad_remote_desktop/assistant/screen_capture.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test(
    'remote frame capture preserves colors and supplies bounded PNG data',
    () async {
      const channel = MethodChannel('aetherdesk/stream_surface');
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(channel, (call) async {
        expect(call.method, 'capture');
        expect(call.arguments, 42);
        return {
          'width': 2,
          'height': 1,
          'rgba': Uint8List.fromList([255, 0, 0, 255, 0, 0, 255, 255]),
        };
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      final capture = await encodeConnectionScreen(
        await AdStreamSurface.capture(42),
        'Design server',
      );
      expect(capture['label'], 'Design server');
      final bytes = base64Decode((capture['url'] as String).split(',').last);
      expect(bytes.length, lessThanOrEqualTo(1024 * 1024));
      final codec = await ui.instantiateImageCodec(bytes);
      final image = (await codec.getNextFrame()).image;
      final rgba = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
      expect(rgba!.buffer.asUint8List(), [255, 0, 0, 255, 0, 0, 255, 255]);
      image.dispose();
      codec.dispose();
    },
  );
}
