import 'dart:async';
import 'dart:io';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../editor/editor_workspace.dart';
import 'document_session.dart';
import 'file_action_policy.dart';
import 'file_open_controller.dart';
import 'windows_file_properties.dart';

class FileWorkspaceActions {
  FileWorkspaceActions({
    required this.context,
    required this.controller,
    required this.editor,
    required this.showEditor,
    required this.refresh,
    this.navigateRemote,
  });
  final BuildContext Function() context;
  final FileOpenController controller;
  final EditorWorkspaceController editor;
  final FutureOr<void> Function() showEditor;
  final Future<void> Function() refresh;
  final ValueChanged<String>? navigateRemote;
  Future<void> run(Future<void> Function() action) async {
    try {
      await action();
    } on Object catch (error) {
      final c = context();
      if (c.mounted)
        ScaffoldMessenger.of(c).showSnackBar(
          SnackBar(
            content: Text(error.toString()),
            duration: const Duration(seconds: 8),
          ),
        );
    }
  }

  Future<bool> confirm(
    String title,
    String message, {
    String accept = 'Continue',
  }) async {
    final c = context();
    if (!c.mounted) return false;
    return await showDialog<bool>(
          context: c,
          builder: (c) => AlertDialog(
            title: Text(title),
            content: SelectableText(message),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(c, false),
                child: const Text('Cancel'),
              ),
              FilledButton(
                onPressed: () => Navigator.pop(c, true),
                child: Text(accept),
              ),
            ],
          ),
        ) ??
        false;
  }

  Future<String?> textPrompt(
    String title,
    String initial, {
    String label = 'Name or path',
  }) async {
    final field = TextEditingController(text: initial);
    final c = context();
    if (!c.mounted) return null;
    final value = await showDialog<String>(
      context: c,
      builder: (c) => AlertDialog(
        title: Text(title),
        content: TextField(
          controller: field,
          autofocus: true,
          decoration: InputDecoration(labelText: label),
          onSubmitted: (s) => Navigator.pop(c, s),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(c),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(c, field.text),
            child: const Text('OK'),
          ),
        ],
      ),
    );
    // The closing route still owns its TextField during its exit animation.
    Future<void>.delayed(const Duration(seconds: 1), field.dispose);
    return value;
  }

  Future<void> open(
    String path, {
    required bool remote,
    bool forceEditor = false,
    bool system = false,
    String? application,
  }) async {
    if (remote) {
      final metadata = Map<String, dynamic>.from(
        (await controller.execute({
              'operation': 'stat',
              'path': path,
            }))['metadata']
            as Map,
      );
      if (metadata['kind'] == 'symlink') {
        final target = metadata['canonicalPath'] as String;
        if (!await confirm(
          'Open link target?',
          '$path points to:\n$target\n\nOpen this target?',
          accept: 'Open target',
        ))
          return;
        final targetMeta = Map<String, dynamic>.from(
          (await controller.execute({
                'operation': 'stat',
                'path': target,
              }))['metadata']
              as Map,
        );
        if (targetMeta['kind'] == 'directory') {
          navigateRemote?.call(target);
          return;
        }
        path = target;
      }
    }
    final doc = remote
        ? await controller.openRemote(path, forceEditor: forceEditor)
        : await controller.openLocal(path, forceEditor: forceEditor);
    if (!system && application == null && doc.editorVisible && !doc.external) {
      await showEditor();
    }
    if (system || application != null || doc.external) {
      try {
        await controller.openApplication(
          doc,
          application: system ? '' : application,
        );
      } on Object catch (error) {
        if (application != null) rethrow;
        if (await confirm(
          'Choose an application',
          '$error\n\nChoose an installed application for ${doc.name}?',
          accept: 'Choose',
        ))
          await openWith(doc);
      }
    }
  }

  Future<void> openWith(DocumentSession doc) async {
    final selected = await openFile(
      confirmButtonText: 'Open with',
      acceptedTypeGroups: Platform.isWindows
          ? const [
              XTypeGroup(label: 'Windows applications', extensions: ['exe']),
            ]
          : const [],
    );
    if (selected == null) return;
    await controller.openApplication(doc, application: selected.path);
    final extension = fileExtension(doc.remotePath);
    if (extension.isNotEmpty &&
        await confirm(
          'Remember application?',
          'Open .$extension files with ${fileName(selected.path)} on this computer?',
          accept: 'Remember',
        ))
      await controller.associations.set(extension, selected.path);
  }

  Future<void> save(DocumentSession doc) async {
    try {
      await controller.save(doc);
      await refresh();
    } on Object catch (error) {
      if (!error.toString().contains('CONFLICT')) rethrow;
      final c = context();
      if (!c.mounted) return;
      final choice = await showDialog<String>(
        context: c,
        builder: (c) => AlertDialog(
          title: const Text('File changed outside this editor'),
          content: Text(
            '${doc.remotePath}\nYour changes are preserved. Choose how to resolve the conflict.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(c),
              child: const Text('Keep changes'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(c, 'compare'),
              child: const Text('Compare'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(c, 'reload'),
              child: const Text('Reload'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(c, 'copy'),
              child: const Text('Save a copy'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(c, 'overwrite'),
              child: const Text('Overwrite'),
            ),
          ],
        ),
      );
      if (choice == 'copy') await saveCopy(doc);
      if (choice == 'compare') await compareCurrent(doc);
      if (choice == 'reload' &&
          await confirm(
            'Discard this draft?',
            'Replace your pending changes with the current file contents?',
            accept: 'Reload',
          ))
        await controller.reload(doc);
      if (choice == 'overwrite' &&
          await confirm(
            'Overwrite current file?',
            'Replace ${doc.remotePath} with your working copy? Changes made outside AetherDesk will be replaced.',
            accept: 'Overwrite',
          )) {
        await controller.save(doc, overwrite: true);
        await refresh();
      }
    }
  }

  Future<void> saveCopy(DocumentSession doc) async {
    final destination = doc.remote
        ? await textPrompt(
            'Save a copy on host',
            '${doc.remotePath}.copy',
            label: 'Host path',
          )
        : (await getSaveLocation(suggestedName: '${doc.name}.copy'))?.path;
    if (destination == null || destination.isEmpty) return;
    await controller.save(doc, destination: destination);
    await refresh();
  }

  Future<void> close(DocumentSession doc) async {
    await editor.flush();
    if (doc.dirty || doc.externalWatching) {
      final c = context();
      if (!c.mounted) return;
      final answer = await showDialog<String>(
        context: c,
        builder: (c) => AlertDialog(
          title: Text('Close ${doc.name}?'),
          content: Text(
            doc.externalWatching
                ? 'Closing stops watching this application for saved changes. Pending changes remain in the working copy.'
                : 'This file has unsaved changes. You can save them or keep a recoverable draft.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(c),
              child: const Text('Cancel'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(c, 'draft'),
              child: const Text('Keep draft and close'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(c, 'save'),
              child: const Text('Save and close'),
            ),
          ],
        ),
      );
      if (answer == null) return;
      if (answer == 'save') {
        await save(doc);
        if (doc.dirty) return;
      }
    }
    await controller.close(doc);
  }

  Future<bool> prepareToClose() async {
    await editor.flush();
    await controller.checkExternalChanges();
    await controller.flushAll();
    if (!controller.hasPendingChanges &&
        !controller.documents.any((d) => d.externalWatching))
      return true;
    return confirm(
      'Disconnect file workspace?',
      'Pending changes have been preserved on this computer. Reopen the same files on this host to recover them.\n\nDisconnecting also stops sending saves from local applications to the host.',
      accept: 'Disconnect',
    );
  }

  Future<void> compareCurrent(DocumentSession doc) async {
    File? temporary;
    try {
      if (doc.remote) {
        temporary = File('${doc.localFile.parent.parent.path}/compare.tmp');
        await controller.execute({
          'operation': 'download',
          'path': doc.remotePath,
          'localPath': temporary.path,
        });
      } else {
        temporary = File(doc.remotePath);
      }
      await _compareFile(
        doc,
        temporary,
        doc.remote ? 'Current host file' : 'Current local file',
      );
    } finally {
      if (doc.remote && temporary != null && await temporary.exists())
        await temporary.delete();
    }
  }

  Future<void> _compareFile(
    DocumentSession doc,
    File other,
    String label,
  ) async {
    final bytes = await other.length() <= maxEditableBytes
        ? await other.readAsBytes()
        : null;
    final content = bytes == null ? null : TextFileContent.decode(bytes);
    if (content != null && doc.editorVisible) {
      controller.select(doc);
      await showEditor();
      await editor.compare(content.text, label);
    } else {
      final equal =
          await controller.digest(other) ==
          await controller.digest(doc.localFile);
      final c = context();
      if (c.mounted)
        await showDialog<void>(
          context: c,
          builder: (c) => AlertDialog(
            title: const Text('File comparison'),
            content: Text(
              equal
                  ? 'The file contents are identical.'
                  : 'The file contents differ. A text comparison is unavailable for binary or large files.',
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(c),
                child: const Text('Close'),
              ),
            ],
          ),
        );
    }
  }

  Future<void> menu({
    required Offset position,
    required String path,
    required bool remote,
    required bool directory,
    required VoidCallback openFolder,
    ValueChanged<String>? insertPath,
  }) async {
    final c = context();
    if (!c.mounted) return;
    final overlay = Overlay.of(c).context.findRenderObject()! as RenderBox;
    final action = await showMenu<String>(
      context: c,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(position.dx, position.dy, 1, 1),
        Offset.zero & overlay.size,
      ),
      items: [
        const PopupMenuItem(value: 'open', child: Text('Open')),
        if (!directory) ...[
          const PopupMenuItem(
            value: 'edit',
            child: Text('Open with built-in editor'),
          ),
          const PopupMenuItem(value: 'with', child: Text('Open with…')),
          const PopupMenuItem(
            value: 'system',
            child: Text('Open with default program'),
          ),
          const PopupMenuItem(
            value: 'compare',
            child: Text('Compare file with…'),
          ),
          const PopupMenuItem(
            value: 'editorDefault',
            child: Text('Use built-in editor for this file type'),
          ),
          if (remote)
            const PopupMenuItem(
              value: 'sendcopy',
              child: Text('Save local copy to host…'),
            ),
        ],
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'rename', child: Text('Rename')),
        const PopupMenuItem(value: 'delete', child: Text('Delete')),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'copy', child: Text('Copy file path')),
        if (remote && insertPath != null)
          const PopupMenuItem(
            value: 'terminal',
            child: Text('Copy file path to terminal'),
          ),
        const PopupMenuItem(value: 'properties', child: Text('Properties')),
        const PopupMenuItem(value: 'permissions', child: Text('Permissions')),
      ],
    );
    if (action == null) return;
    await run(() async {
      if (action == 'copy') {
        await Clipboard.setData(ClipboardData(text: path));
        return;
      }
      if (action == 'terminal') {
        insertPath?.call(quoteTerminalPath(path));
        return;
      }
      if (action == 'open' && directory) {
        openFolder();
        return;
      }
      if ([
        'open',
        'edit',
        'system',
        'with',
        'compare',
        'editorDefault',
      ].contains(action)) {
        if (action == 'editorDefault') {
          await controller.associations.set(fileExtension(path), 'editor');
          await open(path, remote: remote, forceEditor: true);
          return;
        }
        if (action == 'with' || action == 'compare') {
          final doc = remote
              ? await controller.openRemote(path)
              : await controller.openLocal(path);
          if (doc.editorVisible && action == 'compare') await showEditor();
          if (action == 'with') {
            await openWith(doc);
            return;
          }
          final c = context();
          if (!c.mounted) return;
          final side = await showDialog<String>(
            context: c,
            builder: (c) => SimpleDialog(
              title: const Text('Compare with'),
              children: [
                SimpleDialogOption(
                  onPressed: () => Navigator.pop(c, 'local'),
                  child: const Text('A file on this computer'),
                ),
                SimpleDialogOption(
                  onPressed: () => Navigator.pop(c, 'host'),
                  child: const Text('A file on this host'),
                ),
              ],
            ),
          );
          if (side == 'local') {
            final other = await openFile(confirmButtonText: 'Compare');
            if (other != null)
              await _compareFile(doc, File(other.path), fileName(other.path));
          }
          if (side == 'host') {
            final other = await textPrompt(
              'Compare with host file',
              path,
              label: 'Host path',
            );
            if (other != null) {
              final temp = File(
                '${doc.localFile.parent.parent.path}/compare-other.tmp',
              );
              try {
                await controller.execute({
                  'operation': 'download',
                  'path': other,
                  'localPath': temp.path,
                });
                await _compareFile(doc, temp, other);
              } finally {
                if (await temp.exists()) await temp.delete();
              }
            }
          }
          return;
        }
        await open(
          path,
          remote: remote,
          forceEditor: action == 'edit',
          system: action == 'system',
        );
        return;
      }
      if (action == 'sendcopy') {
        final selected = await openFile(
          confirmButtonText: 'Use this local copy',
        );
        if (selected == null) return;
        final target = await textPrompt(
          'Save local copy to host',
          path,
          label: 'Host destination',
        );
        if (target == null || target.isEmpty) return;
        if (target == path) {
          if (!await confirm(
            'Replace host file?',
            'Save ${selected.path} over $path?',
            accept: 'Save to host',
          ))
            return;
          final meta = Map<String, dynamic>.from(
            (await controller.execute({
                  'operation': 'revision',
                  'path': path,
                }))['metadata']
                as Map,
          );
          await controller.execute({
            'operation': 'save',
            'path': path,
            'localPath': selected.path,
            'expectedRevision': meta['revision'],
          });
        } else {
          await controller.execute({
            'operation': 'createCopy',
            'path': target,
            'localPath': selected.path,
          });
        }
        await refresh();
        return;
      }
      if (action == 'properties' || action == 'permissions') {
        await properties(
          path,
          remote: remote,
          permissions: action == 'permissions',
        );
        return;
      }
      String comparablePath(String value) {
        if (remote) return value;
        final absolute = File(value).absolute.path.replaceAll(r'\', '/');
        return Platform.isWindows ? absolute.toLowerCase() : absolute;
      }

      var protectedPath = path;
      if (remote) {
        final metadata = Map<String, dynamic>.from(
          (await controller.execute({
                'operation': 'stat',
                'path': path,
              }))['metadata']
              as Map,
        );
        // Deleting/renaming a link operates on the link itself, never its target.
        if (metadata['kind'] != 'symlink') {
          protectedPath = metadata['canonicalPath'] as String? ?? path;
        }
      } else {
        final kind = await FileSystemEntity.type(path, followLinks: false);
        if (kind == FileSystemEntityType.directory) {
          protectedPath = await Directory(path).resolveSymbolicLinks();
        } else if (kind == FileSystemEntityType.file) {
          protectedPath = await File(path).resolveSymbolicLinks();
        }
      }
      final parentPath = comparablePath(protectedPath);
      if (controller.documents.any((d) {
        if (d.remote != remote) return false;
        final documentPath = comparablePath(d.remotePath);
        return d.remote == remote &&
            (documentPath == parentPath ||
                directory && documentPath.startsWith('$parentPath/'));
      }))
        throw StateError(
          'Close this file or its open child files in the editor before renaming or deleting it.',
        );
      if (action == 'rename') {
        final name = await textPrompt('Rename', fileName(path));
        if (name == null) return;
        if (!validFileName(name))
          throw const FormatException(
            'Enter a single valid file or folder name.',
          );
        if (!remote && safeLocalName(name) != name)
          throw const FormatException('This name cannot be used on Windows.');
        final target = remote
            ? '${path.substring(0, path.lastIndexOf('/') + 1)}$name'
            : '${File(path).parent.path}${Platform.pathSeparator}$name';
        if (remote)
          await controller.execute({
            'operation': 'rename',
            'path': path,
            'destination': target,
          });
        else {
          if (await FileSystemEntity.type(target, followLinks: false) !=
              FileSystemEntityType.notFound)
            throw StateError('The destination already exists.');
          if (directory)
            await Directory(path).rename(target);
          else
            await File(path).rename(target);
        }
      }
      if (action == 'delete' &&
          await confirm(
            'Delete ${directory ? 'folder' : 'file'}?',
            '${directory ? 'Delete this folder and all its contents' : 'Delete this file'}?\n$path',
            accept: 'Delete',
          )) {
        if (remote)
          await controller.execute({
            'operation': 'delete',
            'path': path,
            'recursive': directory,
          });
        else {
          final absolute = File(path).absolute.path;
          if (Directory(absolute).parent.path == absolute)
            throw StateError('Cannot delete a filesystem root.');
          final kind = await FileSystemEntity.type(path, followLinks: false);
          if (kind == FileSystemEntityType.link)
            await Link(path).delete();
          else if (directory)
            await Directory(path).delete(recursive: true);
          else
            await File(path).delete();
        }
      }
      await refresh();
    });
  }

  Future<void> properties(
    String path, {
    required bool remote,
    required bool permissions,
  }) async {
    if (!remote && Platform.isWindows) {
      showWindowsFileProperties(path);
      return;
    }
    final Map<String, dynamic> meta;
    if (remote) {
      meta = Map<String, dynamic>.from(
        (await controller.execute({
              'operation': 'stat',
              'path': path,
            }))['metadata']
            as Map,
      );
    } else {
      final stat = await FileStat.stat(path);
      meta = {
        'path': path,
        'kind': stat.type.toString(),
        'size': stat.size,
        'modified': stat.modified.toString(),
        'permissions': stat.mode,
      };
    }
    if (permissions && remote) {
      final mode = (meta['permissions'] as num?)?.toInt() ?? 0;
      final value = await textPrompt(
        'Host permissions',
        (mode & 0xfff).toRadixString(8).padLeft(3, '0'),
        label: 'Octal permissions (for example 644 or 755)',
      );
      if (value == null) return;
      if (!RegExp(r'^[0-7]{3,4}$').hasMatch(value))
        throw const FormatException('Enter 3 or 4 octal digits.');
      if (meta['kind'] == 'symlink')
        throw StateError(
          'Open the link target explicitly before changing permissions.',
        );
      await controller.execute({
        'operation': 'chmod',
        'path': path,
        'permissions': int.parse(value, radix: 8),
      });
      await refresh();
      return;
    }
    final c = context();
    if (!c.mounted) return;
    await showDialog<void>(
      context: c,
      builder: (c) => AlertDialog(
        title: Text(permissions ? 'Permissions' : 'Properties'),
        content: SingleChildScrollView(
          child: SelectableText(
            [
              for (final entry in meta.entries)
                if (entry.key != 'revision')
                  '${entry.key}: ${entry.key == 'permissions' && entry.value is num ? ((entry.value as num).toInt() & 0xfff).toRadixString(8) : entry.value}',
              if (permissions && !remote)
                'Windows access rights are managed in Windows file properties.',
            ].join('\n'),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(c),
            child: const Text('Close'),
          ),
        ],
      ),
    );
  }
}
