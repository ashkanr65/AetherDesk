import 'dart:io';

import 'package:crypto/crypto.dart';

import 'file_open_controller.dart';
import 'remote_file_repository.dart';

typedef ConfirmTransferReplacement = Future<bool> Function(String path);

/// A bounded, sequential tree transfer. FileOperation keeps the transport's
/// cancellation and atomic host-save protections for every individual file.
class WorkspaceTransfer {
  WorkspaceTransfer({required this.execute});

  final FileOperation execute;
  static const maxDepth = 128;
  static const maxEntries = 100000;

  Future<void> upload(
    String localPath,
    String remotePath, {
    FileJob? job,
    ConfirmTransferReplacement? confirmReplace,
    FileProgressCallback? onProgress,
  }) async {
    job ??= FileJob();
    _checkRemotePath(remotePath);
    final plan = <_TransferEntry>[];
    await _scanLocal(localPath, remotePath, plan, job, 0);
    final total = plan.fold<int>(0, (sum, entry) => sum + entry.size);
    var completed = 0;
    onProgress?.call(0, total);
    for (final entry in plan) {
      _checkCancelled(job);
      await _checkLocalAncestors(entry.local);
      final current = await _remoteMetadata(entry.remote, job);
      if (entry.directory) {
        if (current == null) {
          await execute({'operation': 'mkdir', 'path': entry.remote}, job: job);
        } else if (current['kind'] != 'directory') {
          throw StateError('Cannot merge a folder with ${entry.remote}.');
        }
        continue;
      }
      if (await FileSystemEntity.type(entry.local, followLinks: false) !=
          FileSystemEntityType.file) {
        throw StateError(
          'The source is no longer a regular file: ${entry.local}',
        );
      }
      Map<String, dynamic>? expected;
      if (current != null) {
        if (current['kind'] != 'file') {
          throw StateError(
            'The destination is not a regular file: ${entry.remote}',
          );
        }
        final revision = await execute({
          'operation': 'revision',
          'path': entry.remote,
        }, job: job);
        expected = Map<String, dynamic>.from(revision['metadata'] as Map);
        if (confirmReplace == null || !await confirmReplace(entry.remote)) {
          throw StateError('Replacement cancelled: ${entry.remote}');
        }
      }
      _checkCancelled(job);
      await execute(
        {
          'operation': expected == null ? 'upload' : 'save',
          'path': entry.remote,
          'localPath': entry.local,
          if (expected != null) 'expectedRevision': expected['revision'],
          // A confirmation applies to this revision. A later host edit conflicts.
          'overwrite': false,
        },
        job: job,
        onProgress: (done, _) {
          onProgress?.call(completed + done.clamp(0, entry.size), total);
        },
      );
      completed += entry.size;
      onProgress?.call(completed, total);
    }
    _checkCancelled(job);
  }

  Future<void> download(
    String remotePath,
    String localPath, {
    FileJob? job,
    ConfirmTransferReplacement? confirmReplace,
    FileProgressCallback? onProgress,
  }) async {
    job ??= FileJob();
    _checkRemotePath(remotePath);
    final plan = <_TransferEntry>[];
    await _checkLocalAncestors(localPath);
    await _scanRemote(remotePath, localPath, plan, job, 0);
    final total = plan.fold<int>(0, (sum, entry) => sum + entry.size);
    var completed = 0;
    onProgress?.call(0, total);
    for (final entry in plan) {
      _checkCancelled(job);
      await _checkLocalAncestors(entry.local);
      final kind = await FileSystemEntity.type(entry.local, followLinks: false);
      if (entry.directory) {
        if (kind == FileSystemEntityType.notFound) {
          await Directory(entry.local).create(recursive: true);
        } else if (kind != FileSystemEntityType.directory) {
          throw StateError('Cannot merge a folder with ${entry.local}.');
        }
        continue;
      }
      if (kind != FileSystemEntityType.file &&
          kind != FileSystemEntityType.notFound) {
        throw StateError(
          'The destination is not a regular file: ${entry.local}',
        );
      }
      final target = File(entry.local);
      final before = kind == FileSystemEntityType.file
          ? await _digest(target)
          : null;
      if (before != null &&
          (confirmReplace == null || !await confirmReplace(entry.local))) {
        throw StateError('Replacement cancelled: ${entry.local}');
      }
      _checkCancelled(job);
      await target.parent.create(recursive: true);
      final stage = await target.parent.createTemp('.aetherdesk-transfer-');
      try {
        final staged = File(_joinLocal(stage.path, 'payload'));
        await execute(
          {
            'operation': 'download',
            'path': entry.remote,
            'localPath': staged.path,
          },
          job: job,
          onProgress: (done, _) {
            onProgress?.call(completed + done.clamp(0, entry.size), total);
          },
        );
        _checkCancelled(job);
        await _checkLocalAncestors(entry.local);
        final afterKind = await FileSystemEntity.type(
          entry.local,
          followLinks: false,
        );
        if ((before == null && afterKind != FileSystemEntityType.notFound) ||
            (before != null &&
                (afterKind != FileSystemEntityType.file ||
                    await _digest(target) != before))) {
          throw StateError(
            'CONFLICT: The destination changed during transfer: ${entry.local}',
          );
        }
        await staged.rename(target.path);
      } finally {
        // This unique directory was created here; never recursively delete a
        // user-supplied path or follow a replacement link during cleanup.
        if (await FileSystemEntity.type(stage.path, followLinks: false) ==
            FileSystemEntityType.directory) {
          await stage.delete(recursive: true);
        }
      }
      completed += entry.size;
      onProgress?.call(completed, total);
    }
    _checkCancelled(job);
  }

  Future<void> _scanLocal(
    String local,
    String remote,
    List<_TransferEntry> plan,
    FileJob job,
    int depth,
  ) async {
    _checkBounds(plan, depth, job);
    await _checkLocalAncestors(local);
    final kind = await FileSystemEntity.type(local, followLinks: false);
    if (kind != FileSystemEntityType.file &&
        kind != FileSystemEntityType.directory) {
      throw StateError(
        'Only regular files and folders can be transferred: $local',
      );
    }
    final directory = kind == FileSystemEntityType.directory;
    plan.add(
      _TransferEntry(
        local,
        remote,
        directory,
        directory ? 0 : await File(local).length(),
      ),
    );
    if (!directory) return;
    await for (final child in Directory(local).list(followLinks: false)) {
      final name = child.uri.pathSegments.where((part) => part.isNotEmpty).last;
      _checkName(name);
      await _scanLocal(
        child.path,
        _joinRemote(remote, name),
        plan,
        job,
        depth + 1,
      );
    }
  }

  Future<void> _scanRemote(
    String remote,
    String local,
    List<_TransferEntry> plan,
    FileJob job,
    int depth,
  ) async {
    _checkBounds(plan, depth, job);
    final metadata = await _remoteMetadata(remote, job);
    if (metadata == null ||
        !const ['file', 'directory'].contains(metadata['kind'])) {
      throw StateError(
        'Only regular host files and folders can be transferred: $remote',
      );
    }
    final directory = metadata['kind'] == 'directory';
    plan.add(
      _TransferEntry(
        local,
        remote,
        directory,
        directory ? 0 : (metadata['size'] as num?)?.toInt() ?? 0,
      ),
    );
    if (!directory) return;
    final reply = await execute({
      'operation': 'list',
      'path': remote,
    }, job: job);
    final entries = List<Map>.from(reply['entries'] as List);
    final names = <String>{};
    // Validate every sibling before writing anything, so Windows names cannot
    // flatten distinct host files onto one destination or escape its folder.
    for (final entry in entries) {
      final name = entry['name'] as String;
      _checkName(name, windows: true);
      if (!names.add(name.toLowerCase())) {
        throw StateError('Host filenames collide on Windows: $remote/$name');
      }
    }
    for (final entry in entries) {
      final name = entry['name'] as String;
      await _scanRemote(
        _joinRemote(remote, name),
        _joinLocal(local, name),
        plan,
        job,
        depth + 1,
      );
    }
  }

  Future<Map<String, dynamic>?> _remoteMetadata(
    String path,
    FileJob job,
  ) async {
    _checkCancelled(job);
    try {
      final reply = await execute({
        'operation': 'stat',
        'path': path,
      }, job: job);
      return Map<String, dynamic>.from(reply['metadata'] as Map);
    } on Object catch (error) {
      if (error.toString().contains('file no longer exists')) return null;
      rethrow;
    }
  }

  static Future<void> _checkLocalAncestors(String path) async {
    if (Platform.isWindows) {
      for (final segment in File(path).absolute.uri.pathSegments) {
        if (segment.isEmpty || RegExp(r'^[A-Za-z]:$').hasMatch(segment))
          continue;
        _checkName(segment, windows: true);
      }
    }
    var current = File(path).absolute.path;
    while (true) {
      if (await FileSystemEntity.type(current, followLinks: false) ==
          FileSystemEntityType.link) {
        throw StateError(
          'Transfers do not follow local symbolic links: $current',
        );
      }
      final parent = Directory(current).parent.path;
      if (parent == current) return;
      current = parent;
    }
  }

  static void _checkBounds(List<_TransferEntry> plan, int depth, FileJob job) {
    _checkCancelled(job);
    if (depth > maxDepth || plan.length >= maxEntries) {
      throw StateError(
        'This folder exceeds the transfer limit (128 levels or 100,000 entries).',
      );
    }
  }

  static void _checkCancelled(FileJob job) {
    if (job.cancelled)
      throw StateError('Transfer cancelled. Completed files are preserved.');
  }

  static void _checkRemotePath(String path) {
    if (path.isEmpty ||
        path.contains('\u0000') ||
        path.split('/').any((part) => part == '..')) {
      throw const FormatException('Invalid host transfer path.');
    }
  }

  static void _checkName(String name, {bool windows = false}) {
    if (name.isEmpty ||
        name == '.' ||
        name == '..' ||
        name.contains(RegExp(r'[/\\\x00]'))) {
      throw const FormatException('Unsafe filename in transferred folder.');
    }
    if (windows &&
        (name.length > 255 ||
            name.contains(RegExp(r'[<>:"|?*\x00-\x1f]')) ||
            name.endsWith('.') ||
            name.endsWith(' ') ||
            RegExp(
              r'^(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)',
              caseSensitive: false,
            ).hasMatch(name))) {
      throw FormatException(
        'This host filename is not supported on Windows: $name',
      );
    }
  }

  static String _joinRemote(String parent, String name) =>
      '${parent.endsWith('/') ? parent : '$parent/'}$name';
  static String _joinLocal(String parent, String name) =>
      '$parent${Platform.pathSeparator}$name';
  static Future<String> _digest(File file) async =>
      (await sha256.bind(file.openRead()).first).toString();
}

class _TransferEntry {
  const _TransferEntry(this.local, this.remote, this.directory, this.size);
  final String local, remote;
  final bool directory;
  final int size;
}
