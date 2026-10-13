import 'dart:async';
import 'dart:io';
import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:window_manager/window_manager.dart';
import '../app_theme.dart';
import '../file_workspace/document_session.dart';
import '../file_workspace/file_action_policy.dart';
import '../file_workspace/file_context_menu.dart';
import '../file_workspace/file_open_controller.dart';
import 'editor_workspace.dart';
import 'editor_popup_close.dart';

typedef EditorPopupRequest =
    Future<Map<String, dynamic>> Function(
      String method,
      Map<String, Object?> arguments,
    );

Duration editorPopupRequestTimeout(String method) =>
    const {'save', 'openRemote', 'openLocal', 'execute'}.contains(method)
    ? const Duration(minutes: 2)
    : const Duration(seconds: 30);

class _PopupChange {
  const _PopupChange(this.text, this.generation);
  final String text;
  final int generation;
}

/// A view of the owner's documents, never a second cache or file watcher.
class PopupFileController extends FileOpenController {
  PopupFileController({
    required super.identity,
    required EditorPopupRequest request,
    this.changeDelay = const Duration(milliseconds: 100),
  }) : _request = request,
       super(
         execute: (arguments, {job, onProgress}) async {
           if (job?.cancelled == true) {
             throw StateError('File operation cancelled');
           }
           final reply = await request('execute', {'request': arguments});
           final error = reply['operationError'];
           if (error != null) throw StateError(error.toString());
           return Map<String, dynamic>.from(reply['result'] as Map);
         },
       );

  final EditorPopupRequest _request;
  final Duration changeDelay;
  final Map<String, _PopupChange> _pending = {};
  final Map<String, int> _savedRevisions = {};
  Future<void> _tail = Future<void>.value();
  Timer? _changeTimer;
  int _revision = -1;
  int _selectionPending = 0;
  bool connected = true;

  @override
  Future<void> initialize() async {}

  DocumentSession? _document(String id) =>
      documents.where((document) => document.id == id).firstOrNull;

  void applySnapshot(Map<String, dynamic> snapshot) {
    if (disposed || snapshot['documents'] is! List) return;
    final revision = (snapshot['revision'] as num?)?.toInt();
    if (revision != null && revision < _revision) return;
    if (revision != null) _revision = revision;
    final incoming = <DocumentSession>[];
    for (final value in snapshot['documents'] as List) {
      final state = Map<String, dynamic>.from(value as Map);
      final id = state['id'] as String;
      final existing = _document(id);
      if (existing == null &&
          (state['text'] is! String || state['savedText'] is! String)) {
        throw const FormatException(
          'A new editor document requires its complete text.',
        );
      }
      final savedRevision = (state['savedRevision'] as num?)?.toInt();
      if (savedRevision != null &&
          !state.containsKey('savedText') &&
          savedRevision != _savedRevisions[id]) {
        throw const FormatException(
          'The editor received an unknown saved revision.',
        );
      }
      final doc =
          existing ??
          DocumentSession(
            id: id,
            remotePath: state['remotePath'] as String,
            localFile: File(state['localPath'] as String),
            metadata: Map<String, dynamic>.from(state['metadata'] as Map),
            remote: state['remote'] as bool? ?? true,
          );
      final generation = (state['generation'] as num?)?.toInt() ?? 0;
      final text = state['text'] as String? ?? doc.text;
      final hasUnacknowledgedText =
          _pending.containsKey(id) && text != doc.text;
      final stale =
          existing != null &&
          (hasUnacknowledgedText ||
              editorSnapshotIsStale(
                doc.generation,
                doc.text,
                generation,
                text,
              ));
      final content = state['content'];
      doc.remotePath = state['remotePath'] as String;
      doc.metadata = Map<String, dynamic>.from(state['metadata'] as Map);
      doc.savedText = state['savedText'] as String? ?? doc.savedText;
      if (savedRevision != null) _savedRevisions[id] = savedRevision;
      doc.content = content is Map
          ? TextFileContent(
              stale ? doc.text : text,
              content['encoding'] as String,
              content['bom'] as bool,
            )
          : null;
      doc.localDigest = state['localDigest'] as String? ?? '';
      doc.externalObservedDigest = state['externalObservedDigest'] as String?;
      doc.externalWatching = state['externalWatching'] as bool? ?? false;
      doc.external = state['external'] as bool? ?? false;
      doc.editorVisible = state['editorVisible'] as bool? ?? false;
      doc.saving = state['saving'] as bool? ?? false;
      if (!stale) {
        doc.text = text;
        doc.generation = generation;
        doc.dirty = state['dirty'] as bool? ?? false;
        doc.status = state['status'] as String? ?? '';
        doc.error = state['error'] as String?;
      } else {
        // A delayed acknowledgement must never erase newer typing.
        doc.dirty = doc.text != doc.savedText;
        if (generation >= doc.generation) {
          doc.status = 'Changes pending';
          doc.error =
              'The owner has a different document revision. Your text is still open here.';
        }
      }
      incoming.add(doc);
    }
    // Keep a locally edited document if an older owner snapshot omitted it.
    for (final doc in documents) {
      if (!incoming.any((item) => item.id == doc.id) &&
          _pending.containsKey(doc.id)) {
        incoming.add(doc);
      }
    }
    documents
      ..clear()
      ..addAll(incoming);
    final selection = snapshot['selectedId'] as String?;
    if (_selectionPending == 0 || selection == selectedId) {
      selectedId = selection;
    }
    if (_document(selectedId ?? '') == null) {
      selectedId = documents.lastOrNull?.id;
    }
    notifyListeners();
  }

  Future<T> _serialized<T>(Future<T> Function() action) {
    final result = Completer<T>();
    _tail = _tail.then((_) async {
      try {
        result.complete(await action());
      } on Object catch (error, stack) {
        result.completeError(error, stack);
      }
    });
    return result.future;
  }

  Future<Map<String, dynamic>> _call(
    String method, [
    Map<String, Object?> arguments = const {},
  ]) async {
    if (!connected) throw StateError('The SSH workspace is disconnected.');
    final reply = await _request(method, {
      ...arguments,
      'knownDocuments': {
        for (final doc in documents)
          doc.id: {
            'generation': doc.generation,
            'savedRevision': _savedRevisions[doc.id] ?? -1,
          },
      },
    });
    applySnapshot(reply);
    final error = reply['operationError'];
    if (error != null) throw StateError(error.toString());
    return reply;
  }

  Future<void> _sendChanges() => _serialized(() async {
    while (_pending.isNotEmpty) {
      final entry = _pending.entries.first;
      final queued = entry.value;
      final document = _document(entry.key);
      // Monaco raises its model generation immediately after changed().
      // Read that final generation when draining, rather than the earlier one.
      final generation = document != null && document.text == queued.text
          ? document.generation
          : queued.generation;
      final change = _PopupChange(queued.text, generation);
      try {
        await _call('change', {
          'id': entry.key,
          'text': change.text,
          'generation': change.generation,
        });
        if (identical(_pending[entry.key], queued)) {
          _pending.remove(entry.key);
        }
      } on Object catch (error) {
        final doc = _document(entry.key);
        if (doc != null) {
          doc.error = 'Could not preserve changes: $error';
          doc.status = 'Changes pending';
          if (!disposed) notifyListeners();
        }
        rethrow;
      }
    }
  });

  @override
  void changed(DocumentSession doc, String text) {
    if (disposed || !connected || doc.text == text) return;
    doc.text = text;
    doc.generation++;
    doc.dirty = text != doc.savedText;
    doc.status = doc.dirty
        ? 'Modified'
        : doc.remote
        ? 'Saved to host'
        : 'Saved';
    doc.error = null;
    _pending[doc.id] = _PopupChange(text, doc.generation);
    _changeTimer?.cancel();
    _changeTimer = Timer(changeDelay, () {
      _changeTimer = null;
      unawaited(_sendChanges().catchError((Object _) {}));
    });
    notifyListeners();
  }

  @override
  void select(DocumentSession doc) {
    if (disposed || !connected) return;
    selectedId = doc.id;
    _selectionPending++;
    notifyListeners();
    unawaited(
      _serialized(() async {
        try {
          await _call('select', {'id': doc.id});
        } on Object catch (error) {
          doc.error = error.toString();
          if (!disposed) notifyListeners();
        } finally {
          _selectionPending--;
        }
      }),
    );
  }

  @override
  Future<void> flushAll() async {
    if (disposed) return;
    _changeTimer?.cancel();
    _changeTimer = null;
    await _sendChanges();
    await _serialized(() async {
      await _call('flushDrafts');
    });
  }

  @override
  Future<void> flush(DocumentSession doc) => flushAll();

  @override
  Future<void> persist(DocumentSession doc) => flushAll();

  Future<DocumentSession> _open(
    String method,
    String path,
    bool forceEditor,
  ) async {
    await flushAll();
    final reply = await _serialized(
      () => _call(method, {'path': path, 'forceEditor': forceEditor}),
    );
    final result = _document(reply['resultId'] as String? ?? selectedId ?? '');
    if (result == null)
      throw StateError('The owner did not open the document.');
    return result;
  }

  @override
  Future<DocumentSession> openRemote(String path, {bool forceEditor = false}) =>
      _open('openRemote', path, forceEditor);

  @override
  Future<DocumentSession> openLocal(String path, {bool forceEditor = false}) =>
      _open('openLocal', path, forceEditor);

  @override
  Future<void> save(
    DocumentSession doc, {
    bool overwrite = false,
    String? destination,
  }) async {
    await flushAll();
    await _serialized(() async {
      await _call('save', {
        'id': doc.id,
        'overwrite': overwrite,
        'destination': destination,
      });
    });
  }

  @override
  Future<void> reload(DocumentSession doc) async {
    await flushAll();
    await _serialized(() async {
      await _call('reload', {'id': doc.id});
    });
  }

  @override
  Future<void> close(DocumentSession doc) async {
    await flushAll();
    await _serialized(() async {
      await _call('closeDocument', {'id': doc.id});
    });
  }

  @override
  Future<void> checkExternalChanges() => _serialized(() async {
    await _call('checkExternalChanges');
  });

  Future<void> refreshOwner() => _serialized(() async {
    await _call('refresh');
  });

  @override
  Future<void> showInEditor(DocumentSession doc) async {
    await _open(doc.remote ? 'openRemote' : 'openLocal', doc.remotePath, true);
  }

  @override
  Future<void> openApplication(
    DocumentSession doc, {
    String? application,
  }) async {
    throw StateError('Open local applications from the file browser.');
  }

  @override
  void dispose() {
    _changeTimer?.cancel();
    super.dispose();
  }
}

Future<void> runEditorPopup(
  WindowController current,
  Map<String, dynamic> configuration,
) async {
  await windowManager.ensureInitialized();
  await windowManager.setPreventClose(true);
  await windowManager.waitUntilReadyToShow(
    WindowOptions(
      size: const Size(1100, 760),
      minimumSize: const Size(620, 420),
      center: true,
      title: configuration['title'] as String? ?? 'AetherDesk Editor',
    ),
  );
  runApp(_EditorPopupApp(current: current, configuration: configuration));
}

class _EditorPopupApp extends StatelessWidget {
  const _EditorPopupApp({required this.current, required this.configuration});
  final WindowController current;
  final Map<String, dynamic> configuration;
  @override
  Widget build(BuildContext context) => MaterialApp(
    debugShowCheckedModeBanner: false,
    theme: aetherDeskTheme(Brightness.dark),
    home: _EditorPopup(current: current, configuration: configuration),
  );
}

class _EditorPopup extends StatefulWidget {
  const _EditorPopup({required this.current, required this.configuration});
  final WindowController current;
  final Map<String, dynamic> configuration;
  @override
  State<_EditorPopup> createState() => _EditorPopupState();
}

class _EditorPopupState extends State<_EditorPopup> with WindowListener {
  late final PopupFileController _files;
  late final FileWorkspaceActions _actions;
  final _editor = EditorWorkspaceController();
  final _editorAttached = Completer<void>();
  bool _ready = false, _closing = false, _hostDetached = false;
  bool _nativeClosing = false;
  late final EditorPopupClose _windowClose;
  String? _startupError;
  String? _lastTitle;
  String get _title =>
      'AetherDesk Editor — ${widget.configuration['title'] ?? 'Files'}';
  String get _token => widget.configuration['workspaceToken'] as String;

  Future<Map<String, dynamic>> _request(
    String method,
    Map<String, Object?> arguments,
  ) async {
    final owner = WindowController.fromWindowId(
      widget.configuration['ownerWindowId'] as String,
    );
    final reply = await owner
        .invokeMethod<Object?>('editor.$method', {
          ...arguments,
          'workspaceToken': _token,
          'windowId': widget.current.windowId,
        })
        .timeout(editorPopupRequestTimeout(method));
    if (reply is! Map) throw StateError('The SSH workspace is unavailable.');
    return Map<String, dynamic>.from(reply);
  }

  @override
  void initState() {
    super.initState();
    _files = PopupFileController(
      identity: widget.configuration['identity'] as String,
      request: _request,
    );
    _actions = FileWorkspaceActions(
      context: () => context,
      controller: _files,
      editor: _editor,
      showEditor: () async {},
      refresh: _files.refreshOwner,
    );
    _windowClose = EditorPopupClose(
      shutdownEditor: _editor.shutdown,
      closeNative: () async {
        // close() posts an asynchronous Windows message. Keep the latch set and
        // remove our listener so that message cannot start another draft flush.
        _nativeClosing = true;
        windowManager.removeListener(this);
        try {
          await windowManager.setPreventClose(false);
          await windowManager.close();
        } on Object {
          _nativeClosing = false;
          await windowManager.setPreventClose(true);
          windowManager.addListener(this);
          rethrow;
        }
      },
    );
    _files.addListener(_updateTitle);
    windowManager.addListener(this);
    WidgetsBinding.instance.addPostFrameCallback(
      (_) => unawaited(_initialize()),
    );
  }

  Future<void> _initialize() async {
    try {
      await widget.current.setWindowMethodHandler(_handleOwnerCall);
      final state = await _request('ready', {});
      _files.applySnapshot(state);
      if (!mounted) return;
      setState(() => _ready = true);
      // The popup starts hidden and may already be closing. Attach the editor
      // even when Windows has paused normal frames, so cleanup can await it.
      final binding = WidgetsBinding.instance;
      final attached = binding.endOfFrame;
      binding.scheduleWarmUpFrame();
      await attached;
      if (!_editorAttached.isCompleted) _editorAttached.complete();
      if (!_closing && !_nativeClosing) {
        await windowManager.show();
        if (!_closing && !_nativeClosing) await windowManager.focus();
      }
    } on Object catch (error) {
      if (!mounted) return;
      setState(() => _startupError = error.toString());
      if (!_editorAttached.isCompleted) _editorAttached.complete();
      if (!_closing && !_nativeClosing) await windowManager.show();
    }
  }

  void _updateTitle() {
    final doc = _files.selected;
    final title = doc == null
        ? _title
        : '${doc.dirty ? '*' : ''}${doc.name} — $_title';
    if (title == _lastTitle) return;
    _lastTitle = title;
    unawaited(windowManager.setTitle(title));
  }

  Future<dynamic> _handleOwnerCall(MethodCall call) async {
    final value = call.arguments;
    if (value is! Map || value['workspaceToken'] != _token) {
      throw PlatformException(code: 'invalid_editor_window');
    }
    final args = Map<String, dynamic>.from(value);
    switch (call.method) {
      case 'snapshot':
        _files.applySnapshot(args);
        return true;
      case 'flush':
        await _editorAttached.future;
        await _editor.flush();
        return true;
      case 'compare':
        await _editorAttached.future;
        await _editor.compare(args['text'] as String, args['label'] as String);
        return true;
      case 'focus':
        if (_closing || _nativeClosing) return false;
        await windowManager.show();
        if (_closing || _nativeClosing) return false;
        await windowManager.restore();
        if (_closing || _nativeClosing) return false;
        await windowManager.focus();
        return true;
      case 'disconnect':
        if (_closing) throw StateError('The editor is already closing.');
        setState(() => _closing = true);
        try {
          await windowManager.hide();
          await _editorAttached.future;
          await _editor.flush();
          await _windowClose.prepare();
          _files.connected = false;
          // Complete the RPC before destroying the engine that sends its reply.
          Timer(const Duration(milliseconds: 200), () {
            unawaited(
              _closeNativeWindow().catchError((Object error) async {
                if (!mounted) return;
                await windowManager.show();
                if (mounted) {
                  setState(() => _closing = false);
                  ScaffoldMessenger.of(context).showSnackBar(
                    SnackBar(
                      content: Text(
                        'Could not close the editor window: $error',
                      ),
                    ),
                  );
                }
              }),
            );
          });
          return true;
        } on Object {
          if (mounted) setState(() => _closing = false);
          await windowManager.show();
          rethrow;
        }
      case 'ownerDetached':
        if (mounted) setState(() => _hostDetached = true);
        return true;
      default:
        throw MissingPluginException('Unknown editor method ${call.method}');
    }
  }

  @override
  void onWindowClose() {
    unawaited(_closeWindow());
  }

  Future<void> _closeWindow() async {
    if (_closing || _nativeClosing) return;
    setState(() => _closing = true);
    try {
      if (_ready) {
        await _editor.flush();
        if (!mounted) return;
        if (_files.hasPendingChanges) {
          final choice = await showDialog<String>(
            context: context,
            barrierDismissible: false,
            builder: (dialog) => AlertDialog(
              title: const Text('Close editor?'),
              content: const Text(
                'Some files have unsaved changes. Save them to their original files or keep recoverable drafts on this computer.',
              ),
              actions: [
                TextButton(
                  onPressed: () => Navigator.pop(dialog),
                  child: const Text('Cancel'),
                ),
                TextButton(
                  onPressed: () => Navigator.pop(dialog, 'draft'),
                  child: const Text('Keep drafts and close'),
                ),
                if (!_hostDetached)
                  FilledButton(
                    onPressed: () => Navigator.pop(dialog, 'save'),
                    child: const Text('Save all and close'),
                  ),
              ],
            ),
          );
          if (choice == null) return;
          if (choice == 'save') {
            for (final doc in List<DocumentSession>.of(_files.documents)) {
              if (doc.dirty) await _actions.save(doc);
            }
            if (_files.hasPendingChanges) return;
          }
        }
        await _files.flushAll();
      }
      await windowManager.hide();
      await _editorAttached.future;
      // Take the final snapshot after focus leaves the native window, then
      // release WebView2 before Flutter starts destroying its engine/plugins.
      await _editor.flush();
      await _windowClose.prepare();
      if (_ready) {
        try {
          await _request('closed', {});
        } on Object {
          // The final draft is already acknowledged and plugins are released.
          // Window inventory also tells the owner that this window has closed.
        }
      }
      await _closeNativeWindow();
    } on Object catch (error) {
      await windowManager.show();
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(
              'The editor stayed open to preserve your changes: $error',
            ),
            duration: const Duration(seconds: 10),
          ),
        );
      }
    } finally {
      if (mounted && !_nativeClosing) setState(() => _closing = false);
    }
  }

  Future<void> _closeNativeWindow() => _windowClose.close();

  @override
  void dispose() {
    windowManager.removeListener(this);
    _files.removeListener(_updateTitle);
    _files.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text(_title)),
    body: Column(
      children: [
        if (_hostDetached)
          MaterialBanner(
            content: const Text(
              'The SSH workspace closed. Keep a draft here, then reconnect in the main window to save it.',
            ),
            actions: [
              TextButton(
                onPressed: _closeWindow,
                child: const Text('Close editor'),
              ),
            ],
          ),
        Expanded(
          child: AbsorbPointer(
            absorbing: _closing,
            child: _startupError != null
                ? Center(
                    child: Padding(
                      padding: const EdgeInsets.all(24),
                      child: SelectableText(
                        'Could not open the editor.\n\n$_startupError',
                      ),
                    ),
                  )
                : !_ready
                ? const Center(child: CircularProgressIndicator())
                : EditorWorkspace(
                    controller: _files,
                    viewController: _editor,
                    sessionId: widget.configuration['sessionId'] as String,
                    onSave: (doc) => _actions.run(() async {
                      if (_hostDetached)
                        throw StateError(
                          'Reconnect to the host to save. Your draft is kept on this computer.',
                        );
                      await _actions.save(doc);
                    }),
                    onClose: (doc) => _actions.run(() => _actions.close(doc)),
                    onSaveCopy: (doc) => _actions.run(() async {
                      if (_hostDetached)
                        throw StateError(
                          'Reconnect to the host to save. Your draft is kept on this computer.',
                        );
                      await _actions.saveCopy(doc);
                    }),
                  ),
          ),
        ),
      ],
    ),
  );
}
