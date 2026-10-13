import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'document_session.dart';
import 'file_action_policy.dart';
import 'file_associations.dart';
import 'local_app_launcher.dart';
import 'remote_file_repository.dart';

typedef FileOperation =
    Future<Map<String, dynamic>> Function(
      Map<String, Object?> request, {
      FileJob? job,
      FileProgressCallback? onProgress,
    });

class FileOpenController extends ChangeNotifier {
  FileOpenController({
    required this.identity,
    required this.execute,
    this.launcher = const LocalAppLauncher(),
    Directory? cache,
    FileAssociations? associations,
  }) : associations = associations ?? FileAssociations(),
       cache = cache ?? Directory('${workspaceDataDirectory().path}/copies');
  final String identity;
  final FileOperation execute;
  final LocalAppLauncher launcher;
  final Directory cache;
  final FileAssociations associations;
  final List<DocumentSession> documents = [];
  String? selectedId;
  bool disposed = false;
  Timer? _watchTimer;
  bool _watching = false;
  Future<void> _persistTail = Future.value();
  final Map<String, Future<DocumentSession>> _opening = {};
  final Map<String, Future<void>> _drafts = {};
  final Map<String, Timer> _draftTimers = {};
  DocumentSession? get selected =>
      documents.where((d) => d.id == selectedId).firstOrNull;
  Future<void> initialize() async {
    await associations.load();
    await cache.create(recursive: true);
    if (disposed) return;
    _watchTimer?.cancel();
    _watchTimer = Timer.periodic(
      const Duration(seconds: 2),
      (_) => unawaited(checkExternalChanges()),
    );
  }

  String _id(String peer, String path) => sha256
      .convert(utf8.encode(jsonEncode([identity, peer, path])))
      .toString();
  Future<String> digest(File file) async =>
      (await sha256.bind(file.openRead()).first).toString();

  final Map<
    String,
    ({int size, DateTime modified, DateTime checked, String digest})
  >
  _digestCache = {};

  /// The 2 s external-change watch must not re-hash whole files on every
  /// tick (increase_speed.md D4): recompute only when the size or mtime
  /// changed since the previous digest.
  Future<String> watchedDigest(File file) async {
    final stat = await file.stat();
    final cached = _digestCache[file.path];
    if (cached != null &&
        cached.size == stat.size &&
        cached.modified == stat.modified &&
        DateTime.now().difference(cached.checked) <
            const Duration(seconds: 10)) {
      return cached.digest;
    }
    final value = await digest(file);
    _digestCache[file.path] = (
      size: stat.size,
      modified: stat.modified,
      checked: DateTime.now(),
      digest: value,
    );
    return value;
  }

  void changed(DocumentSession doc, String text) {
    if (doc.text == text) return;
    doc.text = text;
    doc.generation++;
    doc.dirty = text != doc.savedText;
    doc.status = doc.dirty
        ? 'Modified'
        : doc.remote
        ? 'Saved to host'
        : 'Saved';
    doc.error = null;
    _draftTimers.remove(doc.id)?.cancel();
    _draftTimers[doc.id] = Timer(const Duration(milliseconds: 250), () {
      _draftTimers.remove(doc.id);
      unawaited(
        flush(doc).catchError((Object error) {
          doc.error = 'Could not preserve the draft: $error';
          doc.status = 'Draft error';
          if (!disposed) notifyListeners();
        }),
      );
    });
    if (!disposed) notifyListeners();
  }

  Future<void> flush(DocumentSession doc) async {
    _draftTimers.remove(doc.id)?.cancel();
    final previous = _drafts[doc.id] ?? Future<void>.value();
    final bytes = doc.content != null && !doc.external
        ? doc.content!.encode(doc.text)
        : null;
    final work = previous.catchError((Object _) {}).then((_) async {
      if (bytes != null) await doc.localFile.writeAsBytes(bytes, flush: true);
      await persist(doc);
    });
    _drafts[doc.id] = work;
    await work;
  }

  Future<void> flushAll() async {
    for (final doc in List<DocumentSession>.of(documents)) {
      await flush(doc);
    }
  }

  Future<DocumentSession> openRemote(
    String path, {
    bool forceEditor = false,
  }) async {
    final existing = documents
        .where((d) => d.remote && d.remotePath == path)
        .firstOrNull;
    if (existing != null) {
      if (forceEditor) await showInEditor(existing);
      select(existing);
      return existing;
    }
    if (_opening.containsKey(path)) {
      final doc = await _opening[path]!;
      if (forceEditor) await showInEditor(doc);
      return doc;
    }
    final future = _openRemote(path, forceEditor);
    _opening[path] = future;
    try {
      return await future;
    } finally {
      _opening.remove(path);
    }
  }

  Future<DocumentSession> _openRemote(String path, bool forceEditor) async {
    final reply = await execute({'operation': 'stat', 'path': path});
    final meta = Map<String, dynamic>.from(reply['metadata'] as Map);
    if (meta['kind'] == 'symlink')
      throw StateError(
        'This is a link to ${meta['canonicalPath']}. Open the target explicitly to edit it.',
      );
    if (meta['kind'] != 'file')
      throw StateError('Only regular files can be opened.');
    final canonical = meta['canonicalPath'] as String;
    final host = reply['hostIdentity'];
    if (host is! Map ||
        host['host'] is! String ||
        host['port'] is! int ||
        host['username'] is! String ||
        host['keyFingerprint'] is! String)
      throw StateError('The server did not provide a verified host identity.');
    final id = _id(
      jsonEncode([
        host['host'],
        host['port'],
        host['username'],
        host['keyFingerprint'],
      ]),
      canonical,
    );
    final current = documents.where((d) => d.id == id).firstOrNull;
    if (current != null) {
      if (forceEditor) await showInEditor(current);
      select(current);
      return current;
    }
    final directory = Directory('${cache.path}/$id');
    await directory.create(recursive: true);
    final contentDirectory = Directory('${directory.path}/content');
    await contentDirectory.create(recursive: true);
    final local = File(
      '${contentDirectory.path}/${safeLocalName(fileName(canonical))}',
    );
    final manifest = File('${directory.path}/document.json');
    Map<String, dynamic>? recovered;
    if (await local.exists() && !await manifest.exists())
      throw StateError(
        'The recovery record is missing. The working copy has been preserved at ${local.path}.',
      );
    if (await manifest.exists() && await local.exists()) {
      try {
        final value = Map<String, dynamic>.from(
          jsonDecode(await manifest.readAsString()) as Map,
        );
        if (value['identity'] != identity || value['path'] != canonical)
          throw StateError('The recovery record belongs to another file.');
        if (value['lastSavedDigest'] != await digest(local)) recovered = value;
      } on Object {
        throw StateError(
          'The recovery record is unreadable. The working copy has been preserved at ${local.path}.',
        );
      }
    }
    Map<String, dynamic> metadata;
    if (recovered != null) {
      metadata = Map<String, dynamic>.from(recovered['metadata'] as Map);
    } else {
      metadata = await _downloadSnapshot(canonical, local);
    }
    final doc = DocumentSession(
      id: id,
      remotePath: canonical,
      localFile: local,
      metadata: metadata,
    );
    doc.localDigest =
        recovered?['lastSavedDigest'] as String? ?? await digest(local);
    doc.dirty = recovered != null;
    doc.status = doc.dirty ? 'Recovered unsaved changes' : 'Saved to host';
    await persist(doc);
    await _classify(doc, forceEditor: forceEditor);
    documents.add(doc);
    select(doc);
    await persist(doc);
    return doc;
  }

  Future<DocumentSession> openLocal(
    String path, {
    bool forceEditor = false,
  }) async {
    final canonical = await File(path).resolveSymbolicLinks();
    final id = _id('local', canonical);
    final existing = documents.where((d) => d.id == id).firstOrNull;
    if (existing != null) {
      if (forceEditor && !existing.editorVisible) await showInEditor(existing);
      select(existing);
      return existing;
    }
    final directory = Directory('${cache.path}/$id/content');
    await directory.create(recursive: true);
    final local = File(
      '${directory.path}/${safeLocalName(fileName(canonical))}',
    );
    final manifest = File('${directory.parent.path}/document.json');
    Map<String, dynamic>? recovered;
    if (await local.exists() && !await manifest.exists())
      throw StateError(
        'The recovery record is missing. The working copy has been preserved at ${local.path}.',
      );
    if (await manifest.exists() && await local.exists()) {
      final value = Map<String, dynamic>.from(
        jsonDecode(await manifest.readAsString()) as Map,
      );
      if (value['identity'] != identity || value['path'] != canonical)
        throw StateError('The working copy belongs to another file.');
      if (value['lastSavedDigest'] != await digest(local)) recovered = value;
    }
    if (recovered == null) await File(canonical).copy(local.path);
    final doc = DocumentSession(
      id: id,
      remotePath: canonical,
      localFile: local,
      metadata: {},
      remote: false,
    );
    doc.localDigest =
        recovered?['lastSavedDigest'] as String? ?? await digest(local);
    doc.dirty = recovered != null;
    doc.status = doc.dirty ? 'Recovered unsaved changes' : 'Saved';
    await persist(doc);
    await _classify(doc, forceEditor: forceEditor);
    documents.add(doc);
    select(doc);
    await persist(doc);
    return doc;
  }

  Future<void> _classify(
    DocumentSession doc, {
    required bool forceEditor,
  }) async {
    final association = associations.forPath(doc.remotePath);
    if (forceEditor || association == null || association == 'editor') {
      if (await doc.localFile.length() <= maxEditableBytes)
        doc.content = TextFileContent.decode(await doc.localFile.readAsBytes());
    }
    if (forceEditor && doc.content == null)
      throw StateError(
        'This file is binary, has an unsupported encoding, or exceeds the editor size limit.',
      );
    if (doc.content != null) {
      doc.text = doc.content!.text;
      doc.savedText = doc.dirty ? '\u0000' : doc.text;
      doc.editorVisible = true;
    } else {
      doc.external = true;
    }
  }

  Future<void> showInEditor(DocumentSession doc) async {
    if (doc.editorVisible && !doc.external) {
      select(doc);
      return;
    }
    if (!doc.remote && doc.external && !doc.dirty) {
      await File(doc.remotePath).copy(doc.localFile.path);
      doc.localDigest = await digest(doc.localFile);
    }
    if (doc.external && doc.dirty)
      throw StateError(
        'Save pending external changes before switching editors.',
      );
    await _classify(doc, forceEditor: true);
    doc.external = false;
    doc.externalWatching = false;
    select(doc);
  }

  Future<void> openApplication(
    DocumentSession doc, {
    String? application,
  }) async {
    if (doc.dirty && !doc.external)
      throw StateError(
        'Save the editor changes before opening an external application.',
      );
    await flush(doc);
    var target = application ?? associations.forPath(doc.remotePath);
    if (target == 'editor') target = null;
    if (!doc.remote && doc.dirty)
      throw StateError(
        'Resolve the recovered draft before opening this file externally.',
      );
    final observed = await digest(doc.localFile);
    await launcher.open(
      doc.remote ? doc.localFile.path : doc.remotePath,
      application: target,
    );
    doc.external = true;
    doc.editorVisible = false;
    doc.externalWatching = doc.remote;
    doc.externalObservedDigest = observed;
    doc.status = doc.dirty ? 'Pending changes' : 'Open in local application';
    if (!disposed) notifyListeners();
  }

  void select(DocumentSession doc) {
    selectedId = doc.id;
    if (!disposed) notifyListeners();
  }

  Future<void> save(
    DocumentSession doc, {
    bool overwrite = false,
    String? destination,
  }) async {
    if (doc.saveFuture != null) {
      await doc.saveFuture;
      return;
    }
    final work = _save(doc, overwrite: overwrite, destination: destination);
    doc.saveFuture = work;
    try {
      await work;
    } finally {
      doc.saveFuture = null;
    }
  }

  Future<void> _save(
    DocumentSession doc, {
    required bool overwrite,
    String? destination,
  }) async {
    doc.saving = true;
    doc.status = doc.remote ? 'Saving to host' : 'Saving';
    doc.error = null;
    if (!disposed) notifyListeners();
    File? snapshot;
    try {
      final text = doc.text;
      final snapshotBytes = doc.content != null && !doc.external
          ? doc.content!.encode(text)
          : null;
      await flush(doc);
      snapshot = File(
        '${doc.localFile.parent.parent.path}/save-${DateTime.now().microsecondsSinceEpoch}.tmp',
      );
      if (snapshotBytes != null) {
        await snapshot.writeAsBytes(snapshotBytes, flush: true);
      } else {
        final before = await digest(doc.localFile);
        await doc.localFile.copy(snapshot.path);
        if (before != await digest(doc.localFile) ||
            before != await digest(snapshot))
          throw StateError(
            'The external application is still saving this file. Retry after it finishes.',
          );
      }
      final expected = await digest(snapshot);
      if (doc.remote) {
        final result = await execute({
          'operation': destination == null ? 'save' : 'createCopy',
          'path': destination ?? doc.remotePath,
          'localPath': snapshot.path,
          'expectedRevision': doc.metadata['revision'],
          'overwrite': overwrite,
        });
        if (destination == null)
          doc.metadata = Map<String, dynamic>.from(result['metadata'] as Map);
      } else {
        final target = File(destination ?? doc.remotePath);
        if (destination != null && await target.exists())
          throw StateError(
            'The destination already exists. Choose another name.',
          );
        if (destination == null &&
            !overwrite &&
            await digest(target) != doc.localDigest)
          throw StateError(
            'CONFLICT: The local file changed outside AetherDesk.',
          );
        await snapshot.copy(target.path);
      }
      if (destination == null) {
        doc.localDigest = expected;
        doc.savedText = text;
        doc.dirty =
            await digest(doc.localFile) != expected ||
            (!doc.external && doc.text != text);
      }
      doc.status = destination != null
          ? 'Copy saved'
          : doc.dirty
          ? 'Modified'
          : doc.remote
          ? 'Saved to host'
          : 'Saved';
      await persist(doc);
    } on Object catch (error) {
      doc.dirty = true;
      doc.error = error.toString();
      doc.status = doc.error!.contains('CONFLICT')
          ? 'Conflict'
          : 'Changes pending';
      await persist(doc);
      rethrow;
    } finally {
      if (snapshot != null && await snapshot.exists()) await snapshot.delete();
      doc.saving = false;
      if (!disposed) notifyListeners();
    }
  }

  Future<Map<String, dynamic>> _downloadSnapshot(
    String path,
    File destination,
  ) async {
    final temporary = File(
      '${destination.parent.path}/.download-${DateTime.now().microsecondsSinceEpoch}.tmp',
    );
    try {
      final result = await execute({
        'operation': 'download',
        'path': path,
        'localPath': temporary.path,
      });
      final metadata = Map<String, dynamic>.from(result['metadata'] as Map);
      if (await digest(temporary) != metadata['revision'])
        throw StateError('The host file changed while downloading. Try again.');
      await temporary.rename(destination.path);
      return metadata;
    } finally {
      if (await temporary.exists()) await temporary.delete();
    }
  }

  Future<void> reload(DocumentSession doc) async {
    if (doc.saving) await doc.saveFuture;
    await flush(doc);
    if (doc.remote) {
      doc.metadata = await _downloadSnapshot(doc.remotePath, doc.localFile);
    } else {
      final temporary = File(
        '${doc.localFile.parent.path}/.reload-${DateTime.now().microsecondsSinceEpoch}.tmp',
      );
      try {
        await File(doc.remotePath).copy(temporary.path);
        await temporary.rename(doc.localFile.path);
      } finally {
        if (await temporary.exists()) await temporary.delete();
      }
    }
    doc.localDigest = await digest(doc.localFile);
    doc.dirty = false;
    doc.error = null;
    doc.content = null;
    await _classify(doc, forceEditor: doc.editorVisible);
    doc.generation++;
    doc.status = doc.remote ? 'Saved to host' : 'Saved';
    await persist(doc);
    if (!disposed) notifyListeners();
  }

  Future<void> checkExternalChanges() async {
    if (_watching || disposed) return;
    _watching = true;
    try {
      _digestCache.removeWhere(
        (path, _) => !documents.any((doc) => doc.localFile.path == path),
      );
      for (final doc in List<DocumentSession>.of(documents)) {
        if (!doc.externalWatching || doc.saving) continue;
        try {
          if (!await doc.localFile.exists()) continue;
          final first = await watchedDigest(doc.localFile);
          if (first == doc.externalObservedDigest &&
              doc.status != 'Changes pending')
            continue;
          await Future<void>.delayed(const Duration(milliseconds: 600));
          if (disposed ||
              !documents.contains(doc) ||
              !await doc.localFile.exists() ||
              first != await digest(doc.localFile))
            continue;
          doc.externalObservedDigest = first;
          doc.dirty = first != doc.localDigest;
          if (!doc.dirty || doc.status == 'Conflict') {
            await persist(doc);
            if (!disposed) notifyListeners();
            continue;
          }
          await save(doc);
        } on Object catch (error) {
          doc.error = error.toString();
          doc.status = doc.error!.contains('CONFLICT')
              ? 'Conflict'
              : 'Changes pending';
          if (!disposed) notifyListeners();
        }
      }
    } finally {
      _watching = false;
    }
  }

  Future<void> persist(DocumentSession doc) {
    final previous = _persistTail.catchError((Object _) {});
    _persistTail = previous.then((_) async {
      final manifest = File(
        '${doc.localFile.parent.parent.path}/document.json',
      );
      final temporary = File('${manifest.path}.tmp');
      await temporary.writeAsString(
        jsonEncode({
          'identity': identity,
          'path': doc.remotePath,
          'metadata': doc.metadata,
          'lastSavedDigest': doc.localDigest,
          'dirty': doc.dirty,
        }),
        flush: true,
      );
      await temporary.rename(manifest.path);
    });
    return _persistTail;
  }

  Future<void> close(DocumentSession doc) async {
    if (doc.saving) await doc.saveFuture;
    await flush(doc);
    doc.externalWatching = false;
    documents.remove(doc);
    if (selectedId == doc.id) selectedId = documents.lastOrNull?.id;
    if (!disposed) notifyListeners();
  }

  bool get hasPendingChanges => documents.any((doc) => doc.dirty || doc.saving);
  @override
  void dispose() {
    disposed = true;
    _watchTimer?.cancel();
    for (final timer in _draftTimers.values) {
      timer.cancel();
    }
    _draftTimers.clear();
    unawaited(flushAll().catchError((Object _) {}));
    super.dispose();
  }
}
