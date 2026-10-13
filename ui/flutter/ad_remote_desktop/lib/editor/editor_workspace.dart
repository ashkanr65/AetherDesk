import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:webview_windows/webview_windows.dart';
import 'editor_language_service.dart';
import '../file_workspace/file_associations.dart';
import '../file_workspace/document_session.dart';
import '../file_workspace/file_action_policy.dart';
import '../file_workspace/file_open_controller.dart';

bool editorSnapshotIsStale(
  int currentGeneration,
  String currentText,
  int incomingGeneration,
  String incomingText,
) =>
    incomingGeneration < currentGeneration ||
    (incomingGeneration == currentGeneration && incomingText != currentText);

/// Serializes resource creation with teardown. In particular, an editor window
/// cannot destroy its Flutter engine while WebView2 is still being created.
class EditorResourceLifecycle {
  Future<void>? _initialization;
  Future<void>? _shutdown;
  bool _stopping = false;
  bool _closed = false;

  bool get stopping => _stopping;

  Future<void> initialize(Future<void> Function() initialize) {
    if (_stopping) return Future.value();
    return _initialization ??= Future.sync(initialize);
  }

  Future<void> shutdown(Future<void> Function() release) {
    _stopping = true;
    if (_closed) return Future.value();
    return _shutdown ??= _release(release);
  }

  Future<void> _release(Future<void> Function() release) async {
    try {
      try {
        await _initialization;
      } on Object {
        // A failed initialization can still leave other resources to release.
      }
      await release();
      _closed = true;
    } finally {
      _shutdown = null;
    }
  }
}

class EditorWorkspaceController {
  Future<void> Function()? _flush;
  Future<void> Function()? _shutdown;
  Future<void> Function(String, String)? _compare;
  Future<void> flush() async {
    await _flush?.call();
  }

  /// Flush changes before calling this, then await it before closing the native
  /// window. WebView2 must be released while this Flutter engine is still alive.
  Future<void> shutdown() async {
    await _shutdown?.call();
  }

  Future<void> compare(String text, String label) async {
    await _compare?.call(text, label);
  }
}

class EditorWorkspace extends StatefulWidget {
  const EditorWorkspace({
    super.key,
    required this.controller,
    required this.viewController,
    required this.onSave,
    required this.onClose,
    this.onSaveCopy,
    required this.sessionId,
  });
  final FileOpenController controller;
  final String sessionId;
  final EditorWorkspaceController viewController;
  final Future<void> Function(DocumentSession) onSave, onClose;
  final Future<void> Function(DocumentSession)? onSaveCopy;
  @override
  State<EditorWorkspace> createState() => _EditorWorkspaceState();
}

class _EditorWorkspaceState extends State<EditorWorkspace> {
  WebviewController? _web;
  HttpServer? _server;
  StreamSubscription<dynamic>? _messages;
  final _token = List.generate(
    24,
    (_) => Random.secure().nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
  final Map<String, Completer<void>> _flushes = {};
  final _webReady = Completer<void>();
  bool _ready = false, _initializing = false, _fromEditor = false;
  bool _disposed = false, _syncing = false, _syncAgain = false;
  Timer? _readyTimer;
  final Map<String, int> _sentGenerations = {};
  final _resources = EditorResourceLifecycle();
  bool _webDisposed = false;
  Object? _webDisposeError;
  StackTrace? _webDisposeStack;
  Future<void>? _languageStarting;
  Future<void> _disposeWeb(WebviewController web) async {
    if (_webDisposed) return;
    // This plugin marks its controller disposed before the native reply. A
    // second call would silently skip native teardown after a failed reply.
    // Keep that failure blocking native window closure instead of pretending
    // the bridge was released on a subsequent close attempt.
    if (_webDisposeError != null) {
      Error.throwWithStackTrace(_webDisposeError!, _webDisposeStack!);
    }
    try {
      await web.dispose();
      _webDisposed = true;
    } on Object catch (error, stack) {
      _webDisposeError = error;
      _webDisposeStack = stack;
      rethrow;
    }
  }

  String? _error, _lastSync, _comparison, _notice;
  EditorLanguageService? _language;
  String? _languageId;
  String? _languageStatus;
  StreamSubscription<Map<String, dynamic>>? _languageMessages;
  final Map<String, int> _languageVersions = {};
  Future<void> _languageTail = Future.value();
  Timer? _languageTimer;
  Future<void> _configureLanguage() async {
    if (_disposed) return;
    final doc = widget.controller.selected;
    if (doc == null || !doc.remote || !doc.editorVisible) return;
    final language = languageFor(doc.remotePath);
    final settings = File(
      '${workspaceDataDirectory().path}/language-servers.json',
    );
    Map<String, dynamic> stored = {};
    if (await settings.exists()) {
      try {
        stored = Map<String, dynamic>.from(
          jsonDecode(await settings.readAsString()) as Map,
        );
      } on Object {}
    }
    final key = '${widget.controller.identity}:$language';
    final previous = stored[key] as Map?;
    final executable = TextEditingController(
      text: previous?['executable'] as String? ?? '',
    );
    final arguments = TextEditingController(
      text: jsonEncode(previous?['arguments'] ?? []),
    );
    final directory = TextEditingController(
      text:
          previous?['directory'] as String? ??
          doc.remotePath.substring(0, doc.remotePath.lastIndexOf('/')),
    );
    if (!mounted || _disposed) return;
    final start = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: Text('Language support: $language'),
        content: SizedBox(
          width: 520,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Text(
                'Run an installed language server on this host for completion, diagnostics, definitions and symbol rename. Basic editing works without one.',
              ),
              TextField(
                controller: executable,
                decoration: const InputDecoration(
                  labelText: 'Host executable (for example clangd)',
                ),
              ),
              TextField(
                controller: arguments,
                decoration: const InputDecoration(
                  labelText:
                      'Arguments as a JSON list (for example ["--stdio"])',
                ),
              ),
              TextField(
                controller: directory,
                decoration: const InputDecoration(
                  labelText: 'Host project directory',
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(c),
            child: const Text('Cancel'),
          ),
          if (_language != null)
            TextButton(
              onPressed: () => Navigator.pop(c, false),
              child: const Text('Stop'),
            ),
          FilledButton(
            onPressed: () => Navigator.pop(c, true),
            child: const Text('Start'),
          ),
        ],
      ),
    );
    try {
      if (start == null || _disposed) return;
      await _send('languageDeactivate');
      await _languageMessages?.cancel();
      await _language?.dispose();
      _language = null;
      _languageVersions.clear();
      if (_disposed) return;
      if (!start) {
        if (mounted)
          setState(() => _languageStatus = 'Language server stopped');
        return;
      }
      final args = jsonDecode(arguments.text);
      if (args is! List ||
          args.any((v) => v is! String) ||
          executable.text.trim().isEmpty)
        throw const FormatException(
          'Enter an executable and a JSON list of string arguments.',
        );
      final config = {
        'executable': executable.text.trim(),
        'arguments': args,
        'directory': directory.text.trim(),
      };
      stored[key] = config;
      await settings.parent.create(recursive: true);
      await settings.writeAsString(jsonEncode(stored), flush: true);
      if (_disposed) return;
      final service = EditorLanguageService(
        sessionId: widget.sessionId,
        executable: executable.text.trim(),
        arguments: args.cast<String>(),
        workingDirectory: directory.text.trim().isEmpty
            ? null
            : directory.text.trim(),
      );
      _language = service;
      _languageId = language;
      _languageMessages = service.notifications.listen((message) {
        final params = message['params'];
        if (message['method'] == 'textDocument/publishDiagnostics' &&
            params is Map) {
          final document = widget.controller.documents
              .where(
                (d) =>
                    d.remote &&
                    d.editorVisible &&
                    EditorLanguageService.documentUri(d.remotePath) ==
                        params['uri'],
              )
              .firstOrNull;
          final version = params['version'];
          if (document != null &&
              (version is! int || version >= document.generation + 1))
            unawaited(
              _send(
                'diagnostics',
                Map<String, Object?>.from(params),
              ).catchError((Object _) {}),
            );
        }
        if (message['method'] == 'aetherdesk/languageServerError' && mounted)
          setState(() => _languageStatus = params.toString());
      });
      _languageStarting = service.start();
      await _languageStarting;
      if (_disposed) return;
      await _syncLanguage();
      await _send('languageActivate', {'language': language});
      if (mounted)
        setState(() => _languageStatus = '$language language support active');
    } finally {
      Future<void>.delayed(const Duration(seconds: 1), () {
        executable.dispose();
        arguments.dispose();
        directory.dispose();
      });
    }
  }

  Future<void> _syncLanguage() {
    if (_disposed) return Future.value();
    final service = _language;
    if (service == null) return Future.value();
    final docs = widget.controller.documents
        .where(
          (d) =>
              d.remote &&
              d.editorVisible &&
              languageFor(d.remotePath) == _languageId,
        )
        .map(
          (d) => (path: d.remotePath, text: d.text, version: d.generation + 1),
        )
        .toList();
    _languageTail = _languageTail.catchError((Object _) {}).then((_) async {
      if (_disposed || service != _language) return;
      for (final path in List<String>.of(_languageVersions.keys)) {
        if (!docs.any((d) => d.path == path)) {
          await service.closeDocument(path);
          _languageVersions.remove(path);
        }
      }
      for (final doc in docs) {
        if (!_languageVersions.containsKey(doc.path))
          await service.openDocument(
            path: doc.path,
            languageId: _languageId!,
            text: doc.text,
            version: doc.version,
          );
        else if (_languageVersions[doc.path] != doc.version)
          await service.changeDocument(
            path: doc.path,
            text: doc.text,
            version: doc.version,
          );
        _languageVersions[doc.path] = doc.version;
      }
    });
    return _languageTail;
  }

  void _queueLanguage() {
    if (_disposed) return;
    _languageTimer?.cancel();
    _languageTimer = Timer(
      const Duration(milliseconds: 250),
      () => unawaited(
        _syncLanguage().catchError((Object error) {
          if (mounted) setState(() => _languageStatus = error.toString());
        }),
      ),
    );
  }

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_changed);
    widget.viewController._flush = _flush;
    widget.viewController._shutdown = _shutdown;
    widget.viewController._compare = _compare;
  }

  void _changed() {
    if (!mounted || _disposed) return;
    setState(() {});
    if (!_fromEditor)
      unawaited(
        _sync().catchError((Object error) {
          if (mounted) setState(() => _error = error.toString());
        }),
      );
    _queueLanguage();
  }

  Future<void> _initialize() => _resources.initialize(_initializeResources);

  Future<void> _initializeResources() async {
    if (_disposed || _initializing || _web != null) return;
    _initializing = true;
    try {
      if (!Platform.isWindows)
        throw StateError(
          'The embedded editor currently requires Windows. Use Open with to edit this file.',
        );
      if (await WebviewController.getWebViewVersion() == null)
        throw StateError(
          'Install the Microsoft WebView2 Runtime included with AetherDesk, then reopen this connection.',
        );
      if (_disposed) return;
      final manifest =
          jsonDecode(await rootBundle.loadString('assets/editor/manifest.json'))
              as Map;
      final allowed = (manifest['files'] as List).cast<String>().toSet();
      if (_disposed) return;
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      _server = server;
      server.listen((request) async {
        try {
          final segments = request.uri.pathSegments;
          if (request.method != 'GET' ||
              segments.length != 2 ||
              segments.first != _token ||
              !allowed.contains(segments.last)) {
            request.response.statusCode = 404;
            await request.response.close();
            return;
          }
          final name = segments.last;
          final data = await rootBundle.load('assets/editor/$name');
          request.response.headers.set('Cache-Control', 'no-store');
          request.response.headers.set('X-Content-Type-Options', 'nosniff');
          request.response.headers.set(
            'Content-Type',
            name.endsWith('.js')
                ? 'text/javascript; charset=utf-8'
                : name.endsWith('.css')
                ? 'text/css; charset=utf-8'
                : name.endsWith('.html')
                ? 'text/html; charset=utf-8'
                : name.endsWith('.ttf')
                ? 'font/ttf'
                : 'application/octet-stream',
          );
          request.response.add(
            data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes),
          );
          await request.response.close();
        } on Object {
          request.response.statusCode = 500;
          await request.response.close();
        }
      });
      if (!mounted || _disposed) return;
      final web = WebviewController();
      _web = web;
      await web.initialize();
      if (!mounted || _disposed) return;
      await web.setPopupWindowPolicy(WebviewPopupWindowPolicy.deny);
      if (!mounted || _disposed) return;
      _messages = web.webMessage.listen((value) {
        unawaited(
          _receive(value).catchError((Object error) {
            if (mounted) setState(() => _error = error.toString());
          }),
        );
      });
      _readyTimer = Timer(const Duration(seconds: 30), () {
        if (mounted && !_ready) {
          setState(
            () => _error =
                'The editor did not finish loading. Your working copies are preserved; reconnect to retry.',
          );
          if (!_webReady.isCompleted) _webReady.complete();
        }
      });
      await web.loadUrl(
        'http://127.0.0.1:${server.port}/$_token/index.html#$_token',
      );
      if (mounted) setState(() {});
    } on Object catch (error) {
      if (mounted) setState(() => _error = error.toString());
      if (!_webReady.isCompleted) _webReady.complete();
    } finally {
      _initializing = false;
    }
  }

  Future<void> _send(
    String type, [
    Map<String, Object?> values = const {},
  ]) async {
    if (_disposed || _web == null || !_ready) return;
    await _web!.postWebMessage(
      jsonEncode({'version': 1, 'token': _token, 'type': type, ...values}),
    );
  }

  Future<void> _sync() async {
    if (!_ready || _disposed) return;
    if (_syncing) {
      _syncAgain = true;
      return;
    }
    _syncing = true;
    try {
      do {
        _syncAgain = false;
        final docs = widget.controller.documents
            .where((d) => d.editorVisible)
            .toList();
        final stamp =
            '${widget.controller.selectedId}:${docs.map((d) => '${d.id}:${d.generation}').join(',')}';
        if (_lastSync == stamp) continue;
        for (final d in docs) {
          if (_sentGenerations[d.id] == d.generation) continue;
          final generation = d.generation;
          await _send('syncDocument', {
            'document': {
              'id': d.id,
              'name': d.name,
              'remote': d.remote,
              'text': d.text,
              'generation': generation,
              'language': languageFor(d.remotePath),
              'uri': d.remote
                  ? EditorLanguageService.documentUri(d.remotePath)
                  : Uri.file(d.remotePath).toString(),
            },
          });
          _sentGenerations[d.id] = generation;
        }
        await _send('syncSelection', {
          'selectedId': widget.controller.selectedId,
          'ids': docs.map((d) => d.id).toList(),
        });
        _sentGenerations.removeWhere((id, _) => !docs.any((d) => d.id == id));
        _lastSync = stamp;
      } while (_syncAgain && !_disposed);
    } finally {
      _syncing = false;
    }
  }

  Future<void> _receive(dynamic raw) async {
    if (_disposed) return;
    final dynamic decoded = raw is String ? jsonDecode(raw) : raw;
    if (decoded is! Map ||
        decoded['token'] != _token ||
        decoded['version'] != 1)
      return;
    final message = Map<String, dynamic>.from(decoded);
    if (message['type'] == 'ready') {
      _readyTimer?.cancel();
      _ready = true;
      _error = null;
      _lastSync = null;
      _sentGenerations.clear();
      await _sync();
      if (!_webReady.isCompleted) _webReady.complete();
      if (mounted) setState(() {});
      return;
    }
    if (message['type'] == 'editorNotice') {
      if (mounted) setState(() => _notice = message['message']?.toString());
      return;
    }
    if (message['type'] == 'languageRequest') {
      final requestId = message['requestId'];
      try {
        final params = Map<String, Object?>.from(message['params'] as Map);
        final textDocument = params['textDocument'];
        final uri = textDocument is Map ? textDocument['uri'] : null;
        if (_language == null ||
            !widget.controller.documents.any(
              (d) =>
                  d.remote &&
                  d.editorVisible &&
                  languageFor(d.remotePath) == _languageId &&
                  EditorLanguageService.documentUri(d.remotePath) == uri,
            ))
          throw StateError(
            'Open a host document and start its language server first.',
          );
        if (![
          'textDocument/completion',
          'textDocument/definition',
          'textDocument/hover',
          'textDocument/rename',
          'textDocument/formatting',
        ].contains(message['method']))
          throw StateError('Unsupported language request');
        await _syncLanguage();
        final result = await _language!.request(
          message['method'] as String,
          params,
        );
        await _send('languageResult', {
          'requestId': requestId,
          'result': result,
        });
      } on Object catch (error) {
        await _send('languageResult', {
          'requestId': requestId,
          'error': error.toString(),
        });
      }
      return;
    }
    if (message['type'] == 'openDefinition') {
      final uri = Uri.tryParse(message['uri'] as String? ?? '');
      final source = widget.controller.documents
          .where((d) => d.id == message['sourceId'] && d.editorVisible)
          .firstOrNull;
      if (source != null &&
          uri != null &&
          uri.scheme == 'file' &&
          uri.host.isEmpty &&
          uri.path.startsWith('/')) {
        final opened = source.remote
            ? await widget.controller.openRemote(
                uri.toFilePath(windows: false),
                forceEditor: true,
              )
            : await widget.controller.openLocal(
                uri.toFilePath(windows: Platform.isWindows),
                forceEditor: true,
              );
        await _sync();
        if (message['selection'] is Map)
          await _send('navigate', {
            'id': opened.id,
            'selection': message['selection'],
          });
      }
      return;
    }
    final doc = widget.controller.documents
        .where((d) => d.id == message['id'])
        .firstOrNull;
    if (doc == null) return;
    if (['change', 'save', 'close', 'flushed'].contains(message['type'])) {
      final text = message['text'];
      final generation = message['generation'];
      if (text is! String ||
          generation is! int ||
          generation < 0 ||
          text.length > 16 * 1024 * 1024)
        throw StateError('Invalid editor document snapshot');
      // A late flush or previous model snapshot must not replace newer edits or
      // a host reload. Equal revisions must contain the same text.
      final stale = editorSnapshotIsStale(
        doc.generation,
        doc.text,
        generation,
        text,
      );
      if (stale) {
        if (message['type'] == 'flushed')
          _flushes.remove(message['requestId'])?.complete();
        if (message['type'] == 'save' || message['type'] == 'close')
          throw StateError(
            'The editor view changed. Retry after it refreshes.',
          );
        return;
      }
      _notice = null;
      _fromEditor = true;
      try {
        widget.controller.changed(doc, text);
        if (generation > doc.generation) doc.generation = generation;
        _sentGenerations[doc.id] = doc.generation;
      } finally {
        _fromEditor = false;
      }
    }
    switch (message['type']) {
      case 'save':
        await widget.onSave(doc);
        if (_languageVersions.containsKey(doc.remotePath) && !doc.dirty)
          await _language?.saveDocument(doc.remotePath);
      case 'close':
        await widget.onClose(doc);
      case 'select':
        widget.controller.select(doc);
      case 'flushed':
        _flushes.remove(message['requestId'])?.complete();
    }
  }

  Future<void> _flush() async {
    if (!_ready || _disposed) {
      await widget.controller.flushAll();
      return;
    }
    for (final doc in List<DocumentSession>.of(
      widget.controller.documents.where((d) => d.editorVisible),
    )) {
      final id = '${doc.id}-${DateTime.now().microsecondsSinceEpoch}';
      final pending = Completer<void>();
      _flushes[id] = pending;
      final response = pending.future.timeout(const Duration(seconds: 5));
      try {
        await Future.wait<void>([
          _send('flush', {'id': doc.id, 'requestId': id}).catchError((
            Object error,
            StackTrace stack,
          ) {
            if (!pending.isCompleted) pending.completeError(error, stack);
          }),
          response,
        ]);
      } finally {
        _flushes.remove(id);
      }
    }
    await widget.controller.flushAll();
  }

  Future<void> _compare(String text, String label) async {
    if (!_ready) {
      await _initialize();
      await _webReady.future.timeout(const Duration(seconds: 35));
    }
    if (!_ready || _disposed)
      throw StateError(_error ?? 'The editor is not ready for comparison.');
    await _sync();
    await _flush();
    await _send('compare', {'id': widget.controller.selectedId, 'text': text});
    if (mounted) setState(() => _comparison = label);
  }

  Future<void> _action(Future<void> Function() action) async {
    try {
      await action();
    } on Object catch (error) {
      if (mounted)
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(error.toString())));
    }
  }

  Future<void> _shutdown({bool updateView = true}) {
    if (!_disposed) {
      _disposed = true;
      _ready = false;
      if (!_webReady.isCompleted) _webReady.complete();
      _readyTimer?.cancel();
      _languageTimer?.cancel();
      widget.controller.removeListener(_changed);
      for (final pending in _flushes.values) {
        if (!pending.isCompleted) {
          pending.completeError(
            StateError('The editor closed before its changes were confirmed'),
          );
        }
      }
      _flushes.clear();
      if (updateView && mounted) setState(() {});
    }
    return _resources.shutdown(_releaseResources);
  }

  Future<void> _releaseResources() async {
    // Unmount the Webview widget before disposing its controller. The popup
    // caller keeps the engine alive until this complete cleanup acknowledges.
    if (mounted) {
      final binding = WidgetsBinding.instance;
      final frame = binding.endOfFrame;
      // Hidden or minimized windows may receive no vsync. A warm-up frame
      // still unmounts the Webview widget without waiting for the window to
      // become visible again. Capture the future before requesting the frame.
      binding.scheduleWarmUpFrame();
      await frame;
    }

    Object? failure;
    StackTrace? failureStack;
    Future<void> release(Future<void> Function() action) async {
      try {
        await action();
      } on Object catch (error, stack) {
        failure ??= error;
        failureStack ??= stack;
      }
    }

    await release(() async {
      await _messages?.cancel();
      _messages = null;
    });
    await release(() async {
      await _languageMessages?.cancel();
      _languageMessages = null;
    });
    await release(() async {
      try {
        await _languageStarting;
      } on Object {
        // A failed server startup still needs its channel to be closed.
      }
      await _language?.dispose();
      _language = null;
    });
    await release(() async {
      await _server?.close(force: true);
      _server = null;
    });
    await release(() async {
      final web = _web;
      // An initialize failure has no native bridge, and the plugin's dispose
      // waits on its failed initialization future rather than releasing one.
      if (web != null && web.value.isInitialized) await _disposeWeb(web);
    });
    if (failure != null) Error.throwWithStackTrace(failure!, failureStack!);
  }

  @override
  void dispose() {
    // This is a fallback for removal from a widget tree. Native popup closure
    // must await viewController.shutdown() before destroying the engine.
    final shutdown = _shutdown(updateView: false);
    unawaited(shutdown.catchError((Object _) {}));
    widget.viewController._flush = null;
    widget.viewController._compare = null;
    // Removal can precede native window closure (for example after a startup
    // error). The owner must still await the in-flight cleanup in that case.
    widget.viewController._shutdown = () => shutdown;
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_disposed) return const SizedBox.shrink();
    final docs = widget.controller.documents;
    final doc = widget.controller.selected;
    if (docs.any((d) => d.editorVisible) &&
        _web == null &&
        !_initializing &&
        _error == null)
      unawaited(_initialize());
    return Column(
      children: [
        SizedBox(
          height: 40,
          child: ListView(
            scrollDirection: Axis.horizontal,
            children: [
              for (final item in docs)
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 2),
                  child: InputChip(
                    selected: item.id == doc?.id,
                    label: Text('${item.dirty ? '● ' : ''}${item.name}'),
                    onPressed: () => widget.controller.select(item),
                    onDeleted: () => _action(() async {
                      await _flush();
                      await widget.onClose(item);
                    }),
                  ),
                ),
            ],
          ),
        ),
        if (doc != null)
          SizedBox(
            height: 42,
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Row(
                children: [
                  TextButton.icon(
                    onPressed: doc.saving
                        ? null
                        : () => _action(() async {
                            await _flush();
                            await widget.onSave(doc);
                          }),
                    icon: const Icon(Icons.save_outlined),
                    label: Text(doc.remote ? 'Save to host' : 'Save'),
                  ),
                  TextButton(
                    onPressed: widget.onSaveCopy == null
                        ? null
                        : () => _action(() async {
                            await _flush();
                            await widget.onSaveCopy!(doc);
                          }),
                    child: const Text('Save a copy'),
                  ),
                  if (doc.editorVisible) ...[
                    IconButton(
                      tooltip: 'Find',
                      onPressed: () =>
                          _send('command', {'command': 'actions.find'}),
                      icon: const Icon(Icons.search),
                    ),
                    IconButton(
                      tooltip: 'Replace',
                      onPressed: () => _send('command', {
                        'command': 'editor.action.startFindReplaceAction',
                      }),
                      icon: const Icon(Icons.find_replace),
                    ),
                    IconButton(
                      tooltip: 'Format document',
                      onPressed: () => _send('command', {
                        'command': 'editor.action.formatDocument',
                      }),
                      icon: const Icon(Icons.format_align_left),
                    ),
                    IconButton(
                      tooltip: 'Editor commands',
                      onPressed: () => _send('command', {
                        'command': 'editor.action.quickCommand',
                      }),
                      icon: const Icon(Icons.code),
                    ),
                  ],
                  if (doc.remote && doc.editorVisible)
                    TextButton(
                      onPressed: () => _action(_configureLanguage),
                      child: const Text('Language support…'),
                    ),
                  if (_comparison != null)
                    TextButton(
                      onPressed: () {
                        _send('endCompare');
                        setState(() => _comparison = null);
                      },
                      child: Text('Close comparison: $_comparison'),
                    ),
                ],
              ),
            ),
          ),
        if (_notice != null)
          Padding(padding: const EdgeInsets.all(8), child: Text(_notice!)),
        if (_languageStatus != null)
          Text(_languageStatus!, style: const TextStyle(fontSize: 11)),
        if (doc != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Text(
                '${doc.status}${doc.error == null ? '' : ' · ${doc.error}'}',
                maxLines: 3,
                style: TextStyle(
                  fontSize: 12,
                  color: doc.error == null
                      ? null
                      : Theme.of(context).colorScheme.error,
                ),
              ),
            ),
          ),
        Expanded(
          child: Stack(
            children: [
              if (_web?.value.isInitialized ?? false)
                Positioned.fill(
                  child: Offstage(
                    offstage: doc?.editorVisible != true || _error != null,
                    child: Webview(
                      _web!,
                      permissionRequested: (_, kind, userInitiated) =>
                          kind == WebviewPermissionKind.clipboardRead &&
                              userInitiated
                          ? WebviewPermissionDecision.allow
                          : WebviewPermissionDecision.deny,
                    ),
                  ),
                ),
              if (_error != null)
                Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: SelectableText(_error!),
                  ),
                )
              else if (doc == null)
                const Center(child: Text('Double-click a file to open it.'))
              else if (!doc.editorVisible)
                const Center(
                  child: Padding(
                    padding: EdgeInsets.all(24),
                    child: Text(
                      'Opened in a local application.\nSaved changes are sent back to the host while this connection remains open.\nPending changes stay in the local working copy.',
                      textAlign: TextAlign.center,
                    ),
                  ),
                )
              else if (!_ready)
                const Center(child: CircularProgressIndicator()),
            ],
          ),
        ),
      ],
    );
  }
}
