import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter/services.dart';

import '../file_workspace/document_session.dart';
import '../file_workspace/file_action_policy.dart';
import '../file_workspace/file_open_controller.dart';
import 'editor_popup_protocol.dart';
import 'editor_workspace.dart';

/// Owns all file I/O in the main engine. The popup only mirrors editor models.
class EditorPopupOwner {
  EditorPopupOwner(this.files, {required this.refresh});
  final FileOpenController files;
  final Future<void> Function() refresh;
  int _revision = 0;

  final Map<String, String> _savedTexts = {};
  final Map<String, int> _savedRevisions = {};
  int _savedEpoch = 0;
  // Last-sent broadcast text per document, used to omit unchanged texts from
  // later broadcasts (increase_speed.md D5).
  final Map<String, ({String text, String savedText})> _sentDocuments = {};
  bool _sentDeltasReady = false;

  Map<String, dynamic> snapshot({String? resultId, bool broadcast = false}) {
    final state = editorPopupSnapshot(
      files,
      revision: ++_revision,
      resultId: resultId,
    );
    for (final doc in files.documents) {
      if (_savedTexts[doc.id] != doc.savedText) {
        _savedTexts[doc.id] = doc.savedText;
        _savedRevisions[doc.id] = ++_savedEpoch;
      }
    }
    for (final doc in state['documents'] as List) {
      doc['savedRevision'] = _savedRevisions[doc['id']];
    }
    _savedTexts.removeWhere((id, _) => !files.documents.any((d) => d.id == id));
    _savedRevisions.removeWhere((id, _) => !_savedTexts.containsKey(id));
    _applyBroadcastDeltas(state, broadcast);
    return state;
  }

  /// Broadcasts arrive in order, so once one carried a document's text the
  /// popup keeps it until the text changes again. Replies never use these
  /// deltas: a reply that crosses a broadcast must not leave the popup
  /// depending on text the crossed broadcast omitted, so every reply
  /// invalidates the cache and the next broadcast sends full text again.
  void _applyBroadcastDeltas(Map<String, dynamic> state, bool broadcast) {
    final docs = state['documents'] as List;
    final complete = <String, ({String text, String savedText})>{};
    for (final doc in docs) {
      complete[doc['id'] as String] = (
        text: doc['text'] as String,
        savedText: doc['savedText'] as String,
      );
    }
    if (broadcast && _sentDeltasReady) {
      for (final doc in docs) {
        final previous = _sentDocuments[doc['id'] as String];
        if (previous == null) continue;
        if (previous.text == doc['text']) doc.remove('text');
        if (previous.savedText == doc['savedText']) doc.remove('savedText');
      }
    }
    _sentDeltasReady = broadcast;
    _sentDocuments
      ..clear()
      ..addAll(complete);
    // A closed and reopened document must receive its full text again.
    _sentDocuments.removeWhere(
      (id, _) => !files.documents.any((doc) => doc.id == id),
    );
  }

  void invalidateBroadcastDeltas() {
    _sentDeltasReady = false;
    _sentDocuments.clear();
  }

  DocumentSession _document(Map<String, dynamic> args) {
    final id = args['id'];
    final doc = files.documents.where((d) => d.id == id).firstOrNull;
    if (doc == null) throw StateError('This document is no longer open.');
    return doc;
  }

  Future<Map<String, dynamic>> dispatch(
    String method,
    Map<String, dynamic> args,
  ) async {
    try {
      String? resultId;
      Map<String, dynamic>? result;
      switch (method) {
        case 'change':
          final doc = _document(args);
          final text = args['text'];
          final generation = args['generation'];
          if (!doc.editorVisible ||
              doc.external ||
              doc.content == null ||
              text is! String ||
              generation is! int ||
              generation < 0) {
            throw StateError('Invalid editor change.');
          }
          if (editorSnapshotIsStale(
            doc.generation,
            doc.text,
            generation,
            text,
          )) {
            throw StateError(
              'The document changed while the editor was synchronizing. Your popup draft has been kept.',
            );
          }
          if (doc.content!.encode(text).length > maxEditableBytes) {
            throw StateError(
              'The edit exceeds the editor size limit. Your popup draft has been kept.',
            );
          }
          if (text != doc.text) files.changed(doc, text);
          doc.generation = generation;
        case 'select':
          files.select(_document(args));
        case 'openRemote':
          resultId = (await files.openRemote(
            args['path'] as String,
            forceEditor: args['forceEditor'] == true,
          )).id;
        case 'openLocal':
          resultId = (await files.openLocal(
            args['path'] as String,
            forceEditor: args['forceEditor'] == true,
          )).id;
        case 'save':
          await files.save(
            _document(args),
            overwrite: args['overwrite'] == true,
            destination: args['destination'] as String?,
          );
        case 'reload':
          await files.reload(_document(args));
        case 'closeDocument':
          await files.close(_document(args));
        case 'flushDrafts':
          await files.flushAll();
        case 'checkExternalChanges':
          await files.checkExternalChanges();
        case 'refresh':
          await refresh();
        case 'execute':
          final request = Map<String, Object?>.from(args['request'] as Map);
          final operation = request['operation'];
          if (!['stat', 'revision', 'download'].contains(operation)) {
            throw StateError(
              'This operation is unavailable in the editor window.',
            );
          }
          if (operation == 'download') {
            // Comparisons may only write the dedicated comparison copies.
            final destination = File(
              request['localPath'] as String,
            ).absolute.path;
            final allowed = files.documents.any(
              (doc) => ['compare.tmp', 'compare-other.tmp'].any(
                (name) =>
                    File(
                      '${doc.localFile.parent.parent.path}/$name',
                    ).absolute.path ==
                    destination,
              ),
            );
            if (!allowed)
              throw StateError('Invalid editor comparison destination.');
          }
          result = await files.execute(request);
        default:
          throw StateError('Unknown editor request: $method');
      }
      final state = snapshot(resultId: resultId);
      final known = args['knownDocuments'];
      if (['change', 'select', 'flushDrafts'].contains(method) &&
          known is Map) {
        for (final doc in state['documents'] as List) {
          final previous = known[doc['id']];
          if (previous is! Map) continue;
          // Omit only state the requesting engine says it already possesses.
          // Unlike last-sent deltas, this is safe when replies cross broadcasts.
          if (previous['generation'] == doc['generation']) doc.remove('text');
          if (previous['savedRevision'] == doc['savedRevision'])
            doc.remove('savedText');
        }
      }
      return {...state, if (result != null) 'result': result};
    } on Object catch (error) {
      return {...snapshot(), 'operationError': error.toString()};
    }
  }
}

/// One router in the main Flutter engine; each connection has its own popup.
class EditorPopupRouter {
  EditorPopupRouter._(this.window);
  static EditorPopupRouter? _instance;
  final WindowController window;
  final Map<String, EditorPopupHost> _hosts = {};
  StreamSubscription<void>? _windows;

  static Future<void> initialize(WindowController window) async {
    await _instance?._windows?.cancel();
    final router = EditorPopupRouter._(window);
    _instance = router;
    await window.setWindowMethodHandler(router._receive);
    router._windows = onWindowsChanged.listen((_) {
      unawaited(router._checkWindows());
    });
  }

  Future<void> _checkWindows() async {
    try {
      final candidates = {
        for (final host in _hosts.values)
          if (host._window != null) host: host._window!.windowId,
      };
      final ids = (await WindowController.getAll())
          .map((w) => w.windowId)
          .toSet();
      for (final candidate in candidates.entries) {
        if (candidate.key._window?.windowId == candidate.value) {
          candidate.key._checkWindow(ids);
        }
      }
    } on Object {
      // A transient inventory failure must not discard a live editor.
    }
  }

  Future<dynamic> _receive(MethodCall call) async {
    if (!call.method.startsWith('editor.') || call.arguments is! Map) {
      throw PlatformException(code: 'unknown_editor_request');
    }
    final args = Map<String, dynamic>.from(call.arguments as Map);
    final host = _hosts[args['workspaceToken']];
    if (host == null) throw PlatformException(code: 'editor_session_ended');
    return host._receive(call.method.substring(7), args);
  }
}

class EditorPopupHost extends EditorWorkspaceController {
  EditorPopupHost({
    required FileOpenController files,
    required this.sessionId,
    required this.title,
    required Future<void> Function() refresh,
  }) : owner = EditorPopupOwner(files, refresh: refresh) {
    files.addListener(_changed);
  }
  final EditorPopupOwner owner;
  final String sessionId, title;
  static String _newToken() => List.generate(
    24,
    (_) => Random.secure().nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
  String token = _newToken();
  WindowController? _window;
  Completer<void>? _ready;
  Future<void>? _showing;
  Timer? _snapshotTimer;
  bool _creating = false, _ending = false, _detached = false;
  String? _boundWindowId;
  bool _handlingMirrorUpdate = false;

  Future<void> show() {
    if (_ending || _detached)
      return Future.error(StateError('This connection is closing.'));
    return _showing ??= _show().whenComplete(() => _showing = null);
  }

  Future<void> _show() async {
    final router = EditorPopupRouter._instance;
    if (router == null)
      throw StateError(
        'The editor window is unavailable. Rebuild AetherDesk with the updated build kit.',
      );
    if (_window == null) {
      router._hosts.remove(token);
      // Every creation attempt has its own capability; delayed ready/closed
      // messages from a failed engine cannot attach to its replacement.
      token = _newToken();
      router._hosts[token] = this;
      _creating = true;
      _ready = Completer<void>();
      try {
        final window = await WindowController.create(
          WindowConfiguration(
            hiddenAtLaunch: true,
            arguments: jsonEncode({
              'kind': 'editor',
              'workspaceToken': token,
              'ownerWindowId': router.window.windowId,
              'sessionId': sessionId,
              'identity': owner.files.identity,
              'title': title,
            }),
          ),
        );
        if (_boundWindowId != null && _boundWindowId != window.windowId) {
          throw StateError('The editor window could not be identified.');
        }
        _boundWindowId = window.windowId;
        _window = window;
        await _ready!.future.timeout(const Duration(seconds: 25));
      } on Object {
        final failed = _window;
        if (failed != null) {
          try {
            await failed
                .invokeMethod<dynamic>('disconnect', {'workspaceToken': token})
                .timeout(const Duration(seconds: 3));
          } on Object {
            try {
              await failed.hide();
            } on Object {
              /* Already closed. */
            }
          }
        }
        _window = null;
        _boundWindowId = null;
        _ready = null;
        rethrow;
      } finally {
        _creating = false;
      }
    }
    await _call('snapshot', owner.snapshot(broadcast: true));
    await _call('focus');
  }

  Future<dynamic> _receive(String method, Map<String, dynamic> args) async {
    final id = args['windowId'];
    if (id is! String ||
        id.isEmpty ||
        (id != _boundWindowId &&
            !(method == 'ready' && _creating && _boundWindowId == null))) {
      throw PlatformException(code: 'invalid_editor_window');
    }
    if (method == 'ready') {
      _boundWindowId = id;
      if (_ready != null && !_ready!.isCompleted) _ready!.complete();
      return owner.snapshot();
    }
    if (method == 'closed') {
      _window = null;
      _boundWindowId = null;
      _ready = null;
      final reply = owner.snapshot();
      if (_detached) _releaseOwner();
      return reply;
    }
    if (_ending &&
        !['change', 'flushDrafts', 'checkExternalChanges'].contains(method)) {
      return {
        ...owner.snapshot(),
        'operationError': 'This connection is closing.',
      };
    }
    if (['change', 'select', 'flushDrafts'].contains(method)) {
      // The ordered reply already carries this state; avoid another full copy.
      _snapshotTimer?.cancel();
      _handlingMirrorUpdate = true;
      try {
        return await owner.dispatch(method, args);
      } finally {
        _handlingMirrorUpdate = false;
      }
    }
    return owner.dispatch(method, args);
  }

  Future<dynamic> _call(String method, [Map<String, dynamic>? values]) async {
    final window = _window;
    if (window == null) return null;
    return window
        .invokeMethod<dynamic>(method, {...?values, 'workspaceToken': token})
        .timeout(
          Duration(
            seconds: ['flush', 'disconnect'].contains(method)
                ? 120
                : method == 'compare'
                ? 45
                : 15,
          ),
        );
  }

  void _checkWindow(Set<String> ids) {
    if (_creating || _window == null || ids.contains(_window!.windowId)) return;
    _window = null;
    _boundWindowId = null;
    _ready = null;
    if (_detached) _releaseOwner();
  }

  void _changed() {
    if (_ending ||
        _detached ||
        _handlingMirrorUpdate ||
        _window == null ||
        _creating)
      return;
    _snapshotTimer?.cancel();
    _snapshotTimer = Timer(const Duration(milliseconds: 100), () {
      unawaited(
        _call('snapshot', owner.snapshot(broadcast: true)).catchError((
          Object _,
        ) {
          // The popup may not have received the state; later broadcasts must
          // not rely on it.
          owner.invalidateBroadcastDeltas();
          return null;
        }),
      );
    });
  }

  @override
  Future<void> flush() async {
    await _showing;
    if (_window != null) await _call('flush');
    await owner.files.flushAll();
  }

  @override
  Future<void> compare(String text, String label) async {
    await show();
    await _call('compare', {'text': text, 'label': label});
  }

  Future<void> closeForDisconnect() async {
    await _showing;
    _ending = true;
    _snapshotTimer?.cancel();
    try {
      if (_window != null) await _call('disconnect');
      _window = null;
      _boundWindowId = null;
      _ready = null;
      await owner.files.flushAll();
    } finally {
      // A cancelled multi-tab application exit can keep this session open.
      if (!_detached) _ending = false;
    }
  }

  void _releaseOwner() {
    _snapshotTimer?.cancel();
    EditorPopupRouter._instance?._hosts.remove(token);
    if (!owner.files.disposed) owner.files.dispose();
  }

  /// Keep the owner alive until the popup has handed over its final draft.
  Future<void> detach() async {
    if (_detached) return;
    _detached = true;
    owner.files.removeListener(_changed);
    try {
      await closeForDisconnect();
      _releaseOwner();
    } on Object {
      _ending = true;
      // The connection is gone, but change/flush/closed RPCs remain available.
      // Releasing the owner now could destroy the popup's last unsent text.
      if (_window == null) {
        _releaseOwner();
        return;
      }
      try {
        await _call('ownerDetached');
      } on Object {
        /* Keep draft owner alive. */
      }
    }
  }
}
