/// Native window destruction must wait for editor plugins to release their
/// callbacks while the Flutter engine's messenger is still alive.
class EditorPopupClose {
  EditorPopupClose({required this.shutdownEditor, required this.closeNative});

  final Future<void> Function() shutdownEditor;
  final Future<void> Function() closeNative;
  Future<void>? _preparing;
  Future<void>? _closing;

  // Share the actual completion, including a failure. A failed WebView disposal
  // must never turn a second close request into permission to destroy its engine.
  Future<void> prepare() => _preparing ??= shutdownEditor();

  Future<void> close() => _closing ??= _finish().catchError((Object error) {
    _closing = null;
    throw error;
  });

  Future<void> _finish() async {
    await prepare();
    await closeNative();
  }
}
