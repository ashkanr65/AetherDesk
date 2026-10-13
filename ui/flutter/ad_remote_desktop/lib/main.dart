import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';

import 'app.dart';
import 'app_log.dart';
import 'editor/editor_popup_client.dart';
import 'editor/editor_popup_host.dart';

Future<void> main(List<String> arguments) async {
  await runZonedGuarded<Future<void>>(
    () => _start(arguments),
    (error, stack) => AppLog.instance.error(
      AppLogSource.uiDart,
      'Uncaught zone error',
      error: error,
      stack: stack,
    ),
  );
}

Future<void> _start(List<String> arguments) async {
  WidgetsFlutterBinding.ensureInitialized();
  // Persist every uncaught error and its stack trace (log_feature.md C).
  FlutterError.onError = (details) {
    FlutterError.presentError(details);
    AppLog.instance.error(
      AppLogSource.uiDart,
      details.exceptionAsString(),
      error: details.exception,
      stack: details.stack,
    );
  };
  WidgetsBinding.instance.platformDispatcher.onError = (error, stack) {
    AppLog.instance.error(
      AppLogSource.uiDart,
      'Uncaught platform error',
      error: error,
      stack: stack,
    );
    return true;
  };
  final child = arguments.isNotEmpty && arguments.first == 'multi_window';
  if (Platform.isWindows) {
    try {
      final window = await WindowController.fromCurrentEngine();
      final value = window.arguments.isEmpty
          ? null
          : jsonDecode(window.arguments);
      if (value is Map && value['kind'] == 'editor') {
        await runEditorPopup(window, Map<String, dynamic>.from(value));
        return;
      }
      if (child)
        throw StateError('The editor window configuration is missing.');
      await EditorPopupRouter.initialize(window);
    } on Object catch (error) {
      if (child) {
        // An editor plugin failure must not launch another main application.
        runApp(
          MaterialApp(
            home: Scaffold(
              body: Center(
                child: Padding(
                  padding: const EdgeInsets.all(24),
                  child: SelectableText(
                    'Could not open the editor window.\n\n$error',
                  ),
                ),
              ),
            ),
          ),
        );
        return;
      }
      // Keep existing connection features usable if generated plugins are stale.
    }
  }
  runAetherDesk();
}
