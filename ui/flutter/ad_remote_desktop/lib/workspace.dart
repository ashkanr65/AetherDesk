import 'dart:async';
import 'dart:io';
import 'package:flutter/gestures.dart';
import 'editor/editor_popup_host.dart';
import 'file_workspace/file_open_controller.dart';
import 'file_workspace/file_context_menu.dart';
import 'file_workspace/file_action_policy.dart';
import 'file_workspace/remote_file_repository.dart';
import 'file_workspace/transfer_queue.dart';

import 'package:desktop_drop/desktop_drop.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:xterm2/xterm.dart';

import 'native_bridge.dart';
import 'notifications.dart';
import 'remote_directory.dart';
import 'session_store.dart';
import 'terminal_output.dart';
import 'terminal_clipboard.dart';
import 'terminal_shortcuts.dart';
import 'terminal_theme.dart';

typedef WorkspaceAuditCallback =
    void Function(String event, String result, String detail);

/// A live terminal and bidirectional SFTP workspace for one connection tab.
class ConnectedWorkspace extends StatefulWidget {
  const ConnectedWorkspace({
    super.key,
    required this.sessionId,
    required this.profile,
    required this.terminalThemeManager,
    required this.onDisconnect,
    required this.onConnectionLost,
    required this.onRemoteDirectoryChanged,
    this.onAudit,
  });

  final String sessionId;
  final SavedSession profile;
  final TerminalThemeManager terminalThemeManager;
  final VoidCallback onDisconnect;
  final ValueChanged<String> onConnectionLost;
  final ValueChanged<String> onRemoteDirectoryChanged;
  final WorkspaceAuditCallback? onAudit;

  @override
  State<ConnectedWorkspace> createState() => ConnectedWorkspaceState();
}

class ConnectedWorkspaceState extends State<ConnectedWorkspace> {
  bool _explorerVisible = true;
  String _remoteDirectory = '.';
  late final FileOpenController _files;
  late final FileWorkspaceActions _actions;
  late final EditorPopupHost _editor;
  final _explorerKey = GlobalKey<_SftpExplorerState>();
  bool get _sftpOnly =>
      widget.profile.connectionType == ConnectionType.sftpOnly;
  @override
  void initState() {
    super.initState();
    _files = FileOpenController(
      identity:
          '${widget.profile.username}@${widget.profile.host}:${widget.profile.port}',
      execute: RemoteFileRepository(widget.sessionId).execute,
    );
    _editor = EditorPopupHost(
      files: _files,
      sessionId: widget.sessionId,
      title: '${widget.profile.username}@${widget.profile.host}',
      refresh: () async {
        await _explorerKey.currentState?.refresh();
      },
    );
    _actions = FileWorkspaceActions(
      context: () => context,
      controller: _files,
      editor: _editor,
      showEditor: _editor.show,
      refresh: () async {
        await _explorerKey.currentState?.refresh();
      },
      navigateRemote: (path) => _explorerKey.currentState?.navigateRemote(path),
    );
    unawaited(_actions.run(_files.initialize));
  }

  Future<void> assistantOpen(
    String path, {
    required bool remote,
    required bool folder,
  }) async {
    if (!mounted) throw StateError('This connection has closed.');
    if (folder) {
      setState(() => _explorerVisible = true);
      final explorer = _explorerKey.currentState;
      if (explorer == null) throw StateError('The file explorer is not ready.');
      if (remote) {
        await explorer._loadRemote(path: path);
      } else {
        await explorer._loadLocal(path: path);
      }
      if (explorer._error != null) throw StateError(explorer._error!);
    } else {
      await _actions.open(path, remote: remote);
    }
  }

  Future<bool> prepareToClose() async {
    try {
      final ready = await _actions.prepareToClose();
      if (ready) {
        await _explorerKey.currentState?.cancelTransfers();
      }
      return ready;
    } on Object catch (error) {
      if (mounted)
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not preserve file changes: $error')),
        );
      return false;
    }
  }

  Future<bool> finishClosingEditor() async {
    try {
      await _editor.closeForDisconnect();
      return true;
    } on Object catch (error) {
      if (mounted)
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not preserve editor changes: $error')),
        );
      return false;
    }
  }

  void _handleRemoteDirectory(String path) {
    if (path == _remoteDirectory) return;
    setState(() => _remoteDirectory = path);
    widget.onRemoteDirectoryChanged(path);
  }

  void _insertPath(String quoted) {
    if (_sftpOnly) return;
    try {
      NativeBridge().write(widget.sessionId, '$quoted ');
    } on Object catch (error) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(error.toString())));
    }
  }

  @override
  void dispose() {
    unawaited(_editor.detach().catchError((Object _) {}));
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Column(
      children: [
        Material(
          color: colors.surfaceContainerLow,
          child: SizedBox(
            height: 45,
            child: Row(
              children: [
                const SizedBox(width: 10),
                Icon(
                  _sftpOnly
                      ? Icons.folder_copy_outlined
                      : Icons.terminal_rounded,
                  size: 18,
                  color: colors.primary,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    '${widget.profile.connectionType.label} · ${widget.profile.username}@${widget.profile.host}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                TextButton(
                  onPressed: () => unawaited(_actions.run(_editor.show)),
                  child: const Text('Editor'),
                ),
                if (!_sftpOnly)
                  IconButton(
                    tooltip: _explorerVisible
                        ? 'Hide Explorer'
                        : 'SFTP Explorer',
                    onPressed: () =>
                        setState(() => _explorerVisible = !_explorerVisible),
                    icon: const Icon(Icons.folder_open_outlined),
                  ),
                TextButton.icon(
                  onPressed: widget.onDisconnect,
                  icon: const Icon(Icons.link_off_rounded, size: 18),
                  label: const Text('Disconnect'),
                ),
                const SizedBox(width: 8),
              ],
            ),
          ),
        ),
        Expanded(
          child: WorkspaceSplit(
            explorerVisible: _sftpOnly || _explorerVisible,
            explorerOnly: _sftpOnly,
            explorer: SftpExplorer(
              key: _explorerKey,
              sessionId: widget.sessionId,
              terminalDirectory: _remoteDirectory,
              onAudit: widget.onAudit,
              actions: _actions,
              onInsertPath: _sftpOnly ? null : _insertPath,
            ),
            terminal: _sftpOnly
                ? const SizedBox.shrink()
                : TerminalPane(
                    sessionId: widget.sessionId,
                    profile: widget.profile,
                    themeManager: widget.terminalThemeManager,
                    onConnectionLost: widget.onConnectionLost,
                    onRemoteDirectoryChanged: _handleRemoteDirectory,
                  ),
          ),
        ),
      ],
    );
  }
}

/// Keeps the live panes mounted while changing their arrangement or visibility.
class WorkspaceSplit extends StatelessWidget {
  const WorkspaceSplit({
    super.key,
    required this.explorerVisible,
    this.explorerOnly = false,
    required this.explorer,
    required this.terminal,
  });
  final bool explorerVisible;
  final bool explorerOnly;
  final Widget explorer;
  final Widget terminal;
  @override
  Widget build(BuildContext context) => explorerOnly
      ? SizedBox.expand(child: explorer)
      : LayoutBuilder(
          builder: (context, constraints) {
            final horizontal = constraints.maxWidth >= 980;
            return Flex(
              direction: horizontal ? Axis.horizontal : Axis.vertical,
              children: [
                Expanded(
                  flex: explorerVisible ? 4 : 0,
                  child: Offstage(
                    offstage: !explorerVisible,
                    // Offstage lays out its child even with zero flex. Keep it bounded.
                    child: SizedBox(
                      width: constraints.maxWidth * (horizontal ? .4 : 1),
                      height: constraints.maxHeight * (horizontal ? 1 : .4),
                      child: explorer,
                    ),
                  ),
                ),
                Expanded(flex: 6, child: terminal),
              ],
            );
          },
        );
}

class TerminalPane extends StatefulWidget {
  const TerminalPane({
    super.key,
    required this.sessionId,
    required this.profile,
    required this.themeManager,
    required this.onConnectionLost,
    required this.onRemoteDirectoryChanged,
  });

  final String sessionId;
  final SavedSession profile;
  final TerminalThemeManager themeManager;
  final ValueChanged<String> onConnectionLost;
  final ValueChanged<String> onRemoteDirectoryChanged;

  @override
  State<TerminalPane> createState() => _TerminalPaneState();
}

class _TerminalPaneState extends State<TerminalPane> {
  late final NativeBridge _bridge;
  late final Terminal _terminal;
  late final TerminalController _terminalController;
  late final TerminalClipboardInteraction _clipboardInteraction;
  late final TerminalOutputParser _outputParser;
  final RemoteDirectoryTracker _directoryTracker = RemoteDirectoryTracker();
  Timer? _pollTimer;
  int _emptyReads = 0;
  bool _reading = false;
  bool _reportedError = false;

  @override
  void initState() {
    super.initState();
    _bridge = NativeBridge();
    _terminalController = TerminalController();
    _outputParser = TerminalOutputParser();
    _terminal = Terminal(
      maxLines: widget.themeManager.appearance.scrollbackLines,
      onOutput: _send,
      onResize: _resize,
    );
    _clipboardInteraction = TerminalClipboardInteraction(
      terminal: _terminal,
      controller: _terminalController,
      readClipboard: () async =>
          (await Clipboard.getData(Clipboard.kTextPlain))?.text,
      writeClipboard: (text) => Clipboard.setData(ClipboardData(text: text)),
    );
    final palette = widget.themeManager.appearance.palette;
    _terminal.write(
      '${_outputParser.style('AetherDesk', TerminalSemanticRole.authentication, palette)} — '
      '${widget.profile.username}@${widget.profile.host}\r\n',
    );
    _poll();
  }

  void _send(String data) {
    try {
      _bridge.write(widget.sessionId, data);
      _pollTimer?.cancel();
      _pollTimer = Timer(Duration.zero, _poll);
    } on Object catch (error) {
      _showError(error);
    }
  }

  void _resize(int columns, int rows, int pixelWidth, int pixelHeight) {
    try {
      _bridge.resize(widget.sessionId, columns, rows);
    } on Object catch (error) {
      _showError(error);
    }
  }

  void _poll() {
    if (_reading || _reportedError || !mounted) return;
    _reading = true;
    try {
      final output = _bridge.read(widget.sessionId);
      if (output.isNotEmpty) {
        _emptyReads = 0;
        final directoryUpdate = _directoryTracker.process(output);
        for (final path in directoryUpdate.paths) {
          widget.onRemoteDirectoryChanged(path);
        }
        final visibleOutput = directoryUpdate.visibleOutput;
        final appearance = widget.themeManager.appearance;
        if (visibleOutput.isNotEmpty) {
          _terminal.write(
            _outputParser.transform(
              visibleOutput,
              appearance.palette,
              enabled: appearance.autoClassify,
            ),
          );
        }
      } else {
        _emptyReads += 1;
      }
    } on Object catch (error) {
      _showError(error);
    } finally {
      _reading = false;
    }
    if (!_reportedError && mounted) {
      // Back off while the terminal is quiet; 125 Hz ticks per open tab add
      // up (increase_speed.md D1). Active output keeps the fast cadence.
      _pollTimer = Timer(
        Duration(milliseconds: _emptyReads >= 32 ? 32 : 8),
        _poll,
      );
    }
  }

  void _showError(Object error) {
    if (_reportedError || !mounted) return;
    _reportedError = true;
    _pollTimer?.cancel();
    _terminal.write(
      '\r\n${_outputParser.style('Connection error: $error', TerminalSemanticRole.error, widget.themeManager.appearance.palette)}\r\n',
    );
    scheduleMicrotask(() {
      if (mounted) widget.onConnectionLost(error.toString());
    });
  }

  @override
  void dispose() {
    _pollTimer?.cancel();
    _terminalController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: widget.themeManager,
      builder: (context, _) {
        final appearance = widget.themeManager.appearance;
        return ColoredBox(
          color: appearance.palette.background,
          child: Padding(
            padding: const EdgeInsets.all(7),
            child: TerminalView(
              _terminal,
              controller: _terminalController,
              autofocus: true,
              shortcuts: aetherTerminalShortcuts(),
              onSecondaryTapUp: (_, _) {
                unawaited(_clipboardInteraction.handleSecondaryClick());
              },
              backgroundOpacity: 1,
              theme: appearance.palette.xtermTheme,
              textStyle: TerminalStyle(
                fontSize: appearance.fontSize,
                height: appearance.lineHeight,
                fontFamily: 'Cascadia Mono',
                fontFamilyFallback: const ['Consolas', 'monospace'],
                drawBoldTextWithBrightColors: true,
              ),
            ),
          ),
        );
      },
    );
  }
}

enum _TransferDirection { upload, download }

enum _TransferState { running, completed, failed }

class _TransferTask {
  _TransferTask({
    required this.id,
    required this.direction,
    required this.source,
    required this.destination,
    required this.name,
    required this.size,
    required this.resume,
  });

  final String id;
  final _TransferDirection direction;
  final String source;
  final String destination;
  final String name;
  final int? size;
  bool resume;
  FileJob? job;
  int transferred = 0, total = 0;
  _TransferState state = _TransferState.running;
  String? error;
}

enum _ExistingFileAction { replace, resume, cancel }

/// Session operations are shared by the explorer and its transfer queue.
class SftpOperations {
  const SftpOperations();
  Future<List<SftpEntry>> list(String session, String path) =>
      NativeBridge.list(session, path);
  Future<void> upload(
    String session,
    String source,
    String destination, {
    bool resume = false,
  }) => NativeBridge.upload(session, source, destination, resume: resume);
  Future<void> download(
    String session,
    String source,
    String destination, {
    bool resume = false,
  }) => NativeBridge.download(session, source, destination, resume: resume);
}

class SftpExplorer extends StatefulWidget {
  const SftpExplorer({
    super.key,
    required this.sessionId,
    required this.terminalDirectory,
    this.operations = const SftpOperations(),
    this.initialLocalDirectory,
    this.onAudit,
    this.actions,
    this.onInsertPath,
  });

  final String sessionId;
  final String terminalDirectory;
  final SftpOperations operations;
  final String? initialLocalDirectory;
  final WorkspaceAuditCallback? onAudit;
  final FileWorkspaceActions? actions;
  final ValueChanged<String>? onInsertPath;

  @override
  State<SftpExplorer> createState() => _SftpExplorerState();
}

class _SftpExplorerState extends State<SftpExplorer> {
  String _localPath =
      Platform.environment['USERPROFILE'] ?? Directory.current.path;
  String _remotePath = '.';
  List<FileSystemEntity> _localEntries = const [];
  List<SftpEntry> _remoteEntries = const [];
  FileSystemEntity? _selectedLocal;
  SftpEntry? _selectedRemote;
  final List<_TransferTask> _transfers = [];
  Future<void> _transferTail = Future.value();
  bool _listingRemote = false;
  int _localListRequest = 0;
  int _remoteListRequest = 0;
  bool _externalDragActive = false;
  String? _error;

  Future<void> cancelTransfers() async {
    for (final task in _transfers) {
      if (task.state == _TransferState.running) await task.job?.cancel();
    }
  }

  @override
  void dispose() {
    unawaited(cancelTransfers());
    super.dispose();
  }

  Future<void> _queueManaged(
    String source,
    String destination,
    String name,
    _TransferDirection direction,
  ) async {
    final task = _TransferTask(
      id: DateTime.now().microsecondsSinceEpoch.toString(),
      direction: direction,
      source: source,
      destination: destination,
      name: name,
      size: null,
      resume: false,
    )..job = FileJob();
    if (!mounted) return;
    setState(() => _transfers.insert(0, task));
    _transferTail = _transferTail
        .catchError((Object _) {})
        .then((_) => _runTransfer(task));
  }

  void navigateRemote(String path) {
    if (!mounted) return;
    unawaited(_loadRemote(path: path));
  }

  Future<void> refresh() async {
    await Future.wait([_loadLocal(), _loadRemote()]);
  }

  Widget _withMenu(
    Widget row,
    String path,
    bool directory,
    bool remote,
    VoidCallback openFolder,
  ) {
    final actions = widget.actions;
    if (actions == null) return row;
    return Listener(
      onPointerDown: (event) {
        if (remote &&
            event.buttons == kMiddleMouseButton &&
            widget.onInsertPath != null)
          unawaited(
            actions.run(() async {
              widget.onInsertPath!(quoteTerminalPath(path));
            }),
          );
      },
      child: GestureDetector(
        onSecondaryTapUp: (details) => unawaited(
          actions.menu(
            position: details.globalPosition,
            path: path,
            remote: remote,
            directory: directory,
            openFolder: openFolder,
            insertPath: remote ? widget.onInsertPath : null,
          ),
        ),
        child: row,
      ),
    );
  }

  @override
  void initState() {
    super.initState();
    _localPath = widget.initialLocalDirectory ?? _localPath;
    if (widget.terminalDirectory.startsWith('/'))
      _remotePath = widget.terminalDirectory;
    _loadLocal();
    _loadRemote();
  }

  @override
  void didUpdateWidget(covariant SftpExplorer oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.terminalDirectory != oldWidget.terminalDirectory &&
        widget.terminalDirectory.startsWith('/') &&
        widget.terminalDirectory != _remotePath) {
      unawaited(_loadRemote(path: widget.terminalDirectory));
    }
  }

  void _navigateLocalAddress(String value) {
    final path = value.trim();
    if (path.isEmpty) return;
    try {
      // Resolve relative addresses against this pane, never the app's cwd.
      final directory = Directory.fromUri(
        Directory(
          _localPath,
        ).uri.resolveUri(Uri.directory(path, windows: Platform.isWindows)),
      );
      unawaited(_loadLocal(path: directory.path));
    } on Object catch (error) {
      setState(() => _error = error.toString());
    }
  }

  void _navigateRemoteAddress(String value) {
    final path = value.trim();
    if (path.isEmpty) return;
    // SFTP paths are sent directly to the file API, not evaluated by a shell.
    navigateRemote(
      path.startsWith('/') ? path : _joinRemote(_remotePath, path),
    );
  }

  Future<void> _loadLocal({String? path}) async {
    final requestedPath = path ?? _localPath;
    final request = ++_localListRequest;
    try {
      final entries = await Directory(
        requestedPath,
      ).list(followLinks: false).toList();
      entries.sort((left, right) {
        final leftDirectory = left is Directory;
        final rightDirectory = right is Directory;
        if (leftDirectory != rightDirectory) return leftDirectory ? -1 : 1;
        return _baseName(
          left.path,
        ).toLowerCase().compareTo(_baseName(right.path).toLowerCase());
      });
      if (mounted && request == _localListRequest) {
        setState(() {
          if (_localPath != requestedPath) _selectedLocal = null;
          _localPath = requestedPath;
          _localEntries = entries;
          _error = null;
        });
      }
    } on Object catch (error) {
      if (mounted && request == _localListRequest) {
        setState(() => _error = error.toString());
      }
    }
  }

  Future<void> _loadRemote({String? path}) async {
    if (!mounted) return;
    final requestedPath = path ?? _remotePath;
    final request = ++_remoteListRequest;
    setState(() {
      _listingRemote = true;
      _error = null;
    });
    try {
      final entries = await widget.operations.list(
        widget.sessionId,
        requestedPath,
      );
      if (mounted && request == _remoteListRequest) {
        setState(() {
          if (_remotePath != requestedPath) _selectedRemote = null;
          _remotePath = requestedPath;
          _remoteEntries = entries;
        });
      }
    } on Object catch (error) {
      if (mounted && request == _remoteListRequest) {
        setState(() => _error = error.toString());
      }
    } finally {
      if (mounted && request == _remoteListRequest) {
        setState(() => _listingRemote = false);
      }
    }
  }

  Future<void> _uploadSelected() async {
    final files = await openFiles(
      initialDirectory: _localPath,
      confirmButtonText: 'Upload',
    );
    if (!mounted) return;
    for (final file in files) {
      await _queueUpload(file.path);
      if (!mounted) return;
    }
  }

  Future<void> _downloadSelected() async {
    final remote = _selectedRemote;
    if (remote == null || remote.isDirectory) return;
    final destination = await getSaveLocation(
      initialDirectory: _localPath,
      suggestedName: remote.name,
      confirmButtonText: 'Download',
    );
    if (destination == null || !mounted) return;
    await _queueDownload(remote, destination: destination.path);
  }

  Future<void> _queueExternalUploads(List<String> paths) async {
    for (final path in paths) {
      if (widget.actions != null || await FileSystemEntity.isFile(path))
        await _queueUpload(path);
    }
  }

  Future<void> _queueUpload(String localPath) async {
    if (widget.actions != null) {
      await _queueManaged(
        localPath,
        _joinRemote(_remotePath, _baseName(localPath)),
        _baseName(localPath),
        _TransferDirection.upload,
      );
      return;
    }
    final name = _baseName(localPath);
    final target = _joinRemote(_remotePath, name);
    final existing = _remoteEntries.where((entry) => entry.name == name);
    var resume = false;
    if (existing.isNotEmpty) {
      final action = await _chooseExistingFileAction(
        name: name,
        side: 'remote',
        canResume: (existing.first.size ?? 0) < File(localPath).lengthSync(),
      );
      if (action == _ExistingFileAction.cancel) return;
      resume = action == _ExistingFileAction.resume;
    }
    final task = _TransferTask(
      id: DateTime.now().microsecondsSinceEpoch.toString(),
      direction: _TransferDirection.upload,
      source: localPath,
      destination: target,
      name: name,
      size: File(localPath).lengthSync(),
      resume: resume,
    );
    setState(() => _transfers.insert(0, task));
    unawaited(_runTransfer(task));
  }

  Future<void> _queueDownload(SftpEntry remote, {String? destination}) async {
    if (widget.actions != null) {
      await _queueManaged(
        remote.path,
        destination ?? '$_localPath${Platform.pathSeparator}${remote.name}',
        remote.name,
        _TransferDirection.download,
      );
      return;
    }
    final target =
        destination ?? '$_localPath${Platform.pathSeparator}${remote.name}';
    var resume = false;
    if (await File(target).exists()) {
      final existingSize = await File(target).length();
      final action = await _chooseExistingFileAction(
        name: remote.name,
        side: 'local',
        canResume: remote.size == null || existingSize < remote.size!,
      );
      if (action == _ExistingFileAction.cancel) return;
      resume = action == _ExistingFileAction.resume;
    }
    final task = _TransferTask(
      id: DateTime.now().microsecondsSinceEpoch.toString(),
      direction: _TransferDirection.download,
      source: remote.path,
      destination: target,
      name: remote.name,
      size: remote.size,
      resume: resume,
    );
    setState(() => _transfers.insert(0, task));
    unawaited(_runTransfer(task));
  }

  Future<void> _runTransfer(_TransferTask task) async {
    if (mounted) {
      setState(() {
        task
          ..state = _TransferState.running
          ..error = null;
      });
    }
    widget.onAudit?.call(
      'sftp.${task.direction.name}',
      'attempt',
      '${task.source} -> ${task.destination}',
    );
    try {
      if (widget.actions != null) {
        task.job ??= FileJob();
        if (task.job!.cancelled) throw StateError('Transfer cancelled');
        final service = WorkspaceTransfer(
          execute: widget.actions!.controller.execute,
        );
        Future<bool> replace(String path) => widget.actions!.confirm(
          'Replace existing file?',
          path,
          accept: 'Replace',
        );
        void progress(int done, int total) {
          if (mounted)
            setState(() {
              task.transferred = done;
              task.total = total;
            });
        }

        if (task.direction == _TransferDirection.upload)
          await service.upload(
            task.source,
            task.destination,
            job: task.job,
            confirmReplace: replace,
            onProgress: progress,
          );
        else
          await service.download(
            task.source,
            task.destination,
            job: task.job,
            confirmReplace: replace,
            onProgress: progress,
          );
      } else if (task.direction == _TransferDirection.upload) {
        await widget.operations.upload(
          widget.sessionId,
          task.source,
          task.destination,
          resume: task.resume,
        );
      } else {
        await widget.operations.download(
          widget.sessionId,
          task.source,
          task.destination,
          resume: task.resume,
        );
      }
      if (!mounted) return;
      setState(() => task.state = _TransferState.completed);
      widget.onAudit?.call(
        'sftp.${task.direction.name}',
        'success',
        '${task.source} -> ${task.destination}',
      );
      await Future.wait([_loadLocal(), _loadRemote()]);
    } on Object catch (error) {
      if (!mounted) return;
      setState(() {
        task
          ..state = _TransferState.failed
          ..error = error.toString();
      });
      widget.onAudit?.call(
        'sftp.${task.direction.name}',
        'failure',
        error.toString(),
      );
    }
  }

  Future<_ExistingFileAction> _chooseExistingFileAction({
    required String name,
    required String side,
    required bool canResume,
  }) async {
    if (!mounted) return _ExistingFileAction.cancel;
    return await showDialog<_ExistingFileAction>(
          context: context,
          builder: (context) => AlertDialog(
            title: const Text('File already exists'),
            content: Text('$name already exists on the $side side.'),
            actions: [
              TextButton(
                onPressed: () =>
                    Navigator.pop(context, _ExistingFileAction.cancel),
                child: const Text('Cancel'),
              ),
              if (canResume)
                OutlinedButton.icon(
                  onPressed: () =>
                      Navigator.pop(context, _ExistingFileAction.resume),
                  icon: const Icon(Icons.play_arrow_rounded),
                  label: const Text('Resume'),
                ),
              FilledButton(
                onPressed: () =>
                    Navigator.pop(context, _ExistingFileAction.replace),
                child: const Text('Replace'),
              ),
            ],
          ),
        ) ??
        _ExistingFileAction.cancel;
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return DropTarget(
      onDragEntered: (_) => setState(() => _externalDragActive = true),
      onDragExited: (_) => setState(() => _externalDragActive = false),
      onDragDone: (details) {
        setState(() => _externalDragActive = false);
        unawaited(
          _queueExternalUploads(
            details.files.map((file) => file.path).toList(),
          ),
        );
      },
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: colors.surfaceContainerLowest,
          border: _externalDragActive
              ? Border.all(color: colors.primary, width: 2)
              : null,
        ),
        child: Column(
          children: [
            if (_listingRemote) const LinearProgressIndicator(minHeight: 2),
            if (_externalDragActive)
              Container(
                width: double.infinity,
                color: colors.primaryContainer,
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: const Text(
                  'Drop files to upload to the current remote folder',
                  textAlign: TextAlign.center,
                  style: TextStyle(fontWeight: FontWeight.w700),
                ),
              ),
            if (_error != null)
              AutoDismissBanner(
                key: ValueKey(_error),
                message: _error!,
                leading: const Icon(Icons.error_outline_rounded),
                onDismiss: () {
                  if (mounted) setState(() => _error = null);
                },
              ),
            Expanded(
              child: Row(
                children: [
                  Expanded(child: _buildLocalPane()),
                  const VerticalDivider(width: 1),
                  Expanded(child: _buildRemotePane()),
                ],
              ),
            ),
            if (_transfers.isNotEmpty) _buildTransferQueue(),
          ],
        ),
      ),
    );
  }

  Widget _buildLocalPane() {
    return DragTarget<SftpEntry>(
      onAcceptWithDetails: (details) => _queueDownload(details.data),
      builder: (context, candidates, rejects) => _FilePane(
        title: 'Local Files',
        path: _localPath,
        onNavigate: _navigateLocalAddress,
        highlighted: candidates.isNotEmpty,
        leadingAction: IconButton(
          tooltip: 'Parent directory',
          onPressed: () => _loadLocal(path: Directory(_localPath).parent.path),
          icon: const Icon(Icons.arrow_upward_rounded),
        ),
        trailingActions: [
          IconButton(
            tooltip: 'Refresh local files',
            onPressed: _loadLocal,
            icon: const Icon(Icons.refresh_rounded),
          ),
          IconButton.filledTonal(
            tooltip: 'Choose files to upload',
            onPressed: _uploadSelected,
            icon: const Icon(Icons.upload_rounded, size: 18),
          ),
        ],
        child: ListView.builder(
          itemCount: _localEntries.length,
          itemBuilder: (context, index) {
            final entry = _localEntries[index];
            final directory = entry is Directory;
            final selected = entry.path == _selectedLocal?.path;
            final row = _withMenu(
              InkWell(
                onTap: () => setState(() => _selectedLocal = entry),
                onDoubleTap: directory
                    ? () => _loadLocal(path: entry.path)
                    : () => widget.actions?.run(
                        () => widget.actions!.open(entry.path, remote: false),
                      ),
                child: _FileRow(
                  selected: selected,
                  icon: directory
                      ? Icons.folder_rounded
                      : Icons.insert_drive_file_outlined,
                  name: _baseName(entry.path),
                  detail: directory ? 'Folder' : _localSize(entry),
                ),
              ),
              entry.path,
              directory,
              false,
              () => _loadLocal(path: entry.path),
            );
            if (directory && widget.actions == null) return row;
            return Draggable<FileSystemEntity>(
              data: entry,
              feedback: Material(
                elevation: 6,
                borderRadius: BorderRadius.circular(8),
                child: SizedBox(
                  width: 230,
                  child: _FileRow(
                    selected: true,
                    icon: Icons.upload_file_outlined,
                    name: _baseName(entry.path),
                    detail: _localSize(entry),
                  ),
                ),
              ),
              childWhenDragging: Opacity(opacity: .4, child: row),
              child: row,
            );
          },
        ),
      ),
    );
  }

  Widget _buildRemotePane() {
    return DragTarget<FileSystemEntity>(
      onAcceptWithDetails: (details) => _queueUpload(details.data.path),
      builder: (context, candidates, rejects) => _FilePane(
        title: 'Remote Files',
        path: _remotePath,
        onNavigate: _navigateRemoteAddress,
        highlighted: candidates.isNotEmpty,
        leadingAction: IconButton(
          tooltip: 'Parent directory',
          onPressed: () => navigateRemote(_remoteParent(_remotePath)),
          icon: const Icon(Icons.arrow_upward_rounded),
        ),
        trailingActions: [
          IconButton(
            tooltip: 'Refresh remote files',
            onPressed: _loadRemote,
            icon: const Icon(Icons.refresh_rounded),
          ),
          IconButton.filledTonal(
            tooltip: _selectedRemote == null
                ? 'Select a file in Remote Files to download'
                : 'Download ${_selectedRemote!.name}',
            onPressed: _selectedRemote != null && !_selectedRemote!.isDirectory
                ? _downloadSelected
                : null,
            icon: const Icon(Icons.download_rounded, size: 18),
          ),
        ],
        child: ListView.builder(
          itemCount: _remoteEntries.length,
          itemBuilder: (context, index) {
            final entry = _remoteEntries[index];
            final selected = entry.path == _selectedRemote?.path;
            final row = _withMenu(
              InkWell(
                onTapDown: (_) => setState(() => _selectedRemote = entry),
                onTap: () => setState(() => _selectedRemote = entry),
                onDoubleTap: entry.isDirectory
                    ? () => navigateRemote(entry.path)
                    : () => widget.actions?.run(
                        () => widget.actions!.open(entry.path, remote: true),
                      ),
                child: _FileRow(
                  selected: selected,
                  icon: entry.isDirectory
                      ? Icons.folder_rounded
                      : Icons.insert_drive_file_outlined,
                  name: entry.name,
                  detail: entry.isDirectory
                      ? 'Folder'
                      : _formatSize(entry.size),
                ),
              ),
              entry.path,
              entry.isDirectory,
              true,
              () => navigateRemote(entry.path),
            );
            if (entry.isDirectory && widget.actions == null) return row;
            final fileRow = Row(
              children: [
                Expanded(child: row),
                if (!entry.isDirectory)
                  IconButton(
                    tooltip: 'Download ${entry.name}',
                    icon: const Icon(Icons.download_outlined, size: 18),
                    onPressed: () {
                      setState(() => _selectedRemote = entry);
                      unawaited(_downloadSelected());
                    },
                  ),
              ],
            );
            return Draggable<SftpEntry>(
              data: entry,
              feedback: Material(
                elevation: 6,
                borderRadius: BorderRadius.circular(8),
                child: SizedBox(
                  width: 230,
                  child: _FileRow(
                    selected: true,
                    icon: Icons.download_for_offline_outlined,
                    name: entry.name,
                    detail: _formatSize(entry.size),
                  ),
                ),
              ),
              childWhenDragging: Opacity(opacity: .4, child: fileRow),
              child: fileRow,
            );
          },
        ),
      ),
    );
  }

  Widget _buildTransferQueue() {
    final colors = Theme.of(context).colorScheme;
    return Container(
      constraints: const BoxConstraints(maxHeight: 180),
      decoration: BoxDecoration(
        color: colors.surfaceContainerLow,
        border: Border(top: BorderSide(color: colors.outlineVariant)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          SizedBox(
            height: 35,
            child: Row(
              children: [
                const SizedBox(width: 12),
                const Icon(Icons.sync_alt_rounded, size: 17),
                const SizedBox(width: 8),
                Text(
                  'Transfers · ${_transfers.where((task) => task.state == _TransferState.running).length} active',
                  style: const TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                const Spacer(),
                TextButton(
                  onPressed: () => setState(
                    () => _transfers.removeWhere(
                      (task) => task.state == _TransferState.completed,
                    ),
                  ),
                  child: const Text('Clear completed'),
                ),
              ],
            ),
          ),
          Flexible(
            child: ListView.builder(
              itemCount: _transfers.length,
              itemBuilder: (context, index) {
                final task = _transfers[index];
                return SizedBox(
                  height: 52,
                  child: Row(
                    children: [
                      const SizedBox(width: 12),
                      Icon(
                        task.direction == _TransferDirection.upload
                            ? Icons.upload_rounded
                            : Icons.download_rounded,
                        size: 18,
                      ),
                      const SizedBox(width: 9),
                      Expanded(
                        child: Column(
                          mainAxisAlignment: MainAxisAlignment.center,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              task.name,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                            ),
                            const SizedBox(height: 4),
                            if (task.state == _TransferState.running)
                              LinearProgressIndicator(
                                minHeight: 3,
                                value: task.total > 0
                                    ? task.transferred / task.total
                                    : null,
                              )
                            else
                              Text(
                                task.state == _TransferState.completed
                                    ? 'Completed · ${_formatSize(task.size ?? task.total)}'
                                    : task.error ?? 'Transfer failed',
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  color: task.state == _TransferState.failed
                                      ? colors.error
                                      : colors.onSurfaceVariant,
                                  fontSize: 11,
                                ),
                              ),
                          ],
                        ),
                      ),
                      if (task.state == _TransferState.running &&
                          task.job != null)
                        IconButton(
                          tooltip: 'Cancel transfer',
                          onPressed: () => task.job!.cancel(),
                          icon: const Icon(Icons.close),
                        ),
                      if (task.resume)
                        const Padding(
                          padding: EdgeInsets.symmetric(horizontal: 6),
                          child: Chip(
                            label: Text('RESUME'),
                            visualDensity: VisualDensity.compact,
                          ),
                        ),
                      if (task.state == _TransferState.failed)
                        IconButton(
                          tooltip: widget.actions == null
                              ? 'Resume transfer'
                              : 'Retry transfer',
                          onPressed: () {
                            if (widget.actions != null) {
                              task.job = FileJob();
                              _transferTail = _transferTail
                                  .catchError((Object _) {})
                                  .then((_) => _runTransfer(task));
                            } else {
                              task.resume = true;
                              unawaited(_runTransfer(task));
                            }
                          },
                          icon: const Icon(Icons.play_arrow_rounded),
                        ),
                      const SizedBox(width: 6),
                    ],
                  ),
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

class _FilePane extends StatelessWidget {
  const _FilePane({
    required this.title,
    required this.path,
    required this.onNavigate,
    required this.highlighted,
    required this.leadingAction,
    required this.trailingActions,
    required this.child,
  });

  final String title;
  final String path;
  final ValueChanged<String> onNavigate;
  final bool highlighted;
  final Widget leadingAction;
  final List<Widget> trailingActions;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: highlighted
            ? colors.primaryContainer.withValues(alpha: .3)
            : null,
        border: highlighted ? Border.all(color: colors.primary) : null,
      ),
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(8, 7, 7, 6),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    title,
                    style: const TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                ),
                leadingAction,
                ...trailingActions,
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(8, 0, 8, 7),
            child: _FileAddressBar(
              path: path,
              label: '$title address',
              onNavigate: onNavigate,
            ),
          ),
          const Divider(height: 1),
          Expanded(child: child),
        ],
      ),
    );
  }
}

class _FileAddressBar extends StatefulWidget {
  const _FileAddressBar({
    required this.path,
    required this.label,
    required this.onNavigate,
  });
  final String path;
  final String label;
  final ValueChanged<String> onNavigate;

  @override
  State<_FileAddressBar> createState() => _FileAddressBarState();
}

class _FileAddressBarState extends State<_FileAddressBar> {
  late final TextEditingController _controller = TextEditingController(
    text: widget.path,
  );

  void _restorePath() {
    _controller.value = TextEditingValue(
      text: widget.path,
      selection: TextSelection.collapsed(offset: widget.path.length),
    );
  }

  @override
  void didUpdateWidget(covariant _FileAddressBar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.path != widget.path) _restorePath();
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Semantics(
      label: widget.label,
      child: Focus(
        onKeyEvent: (node, event) {
          if (event is KeyDownEvent &&
              event.logicalKey == LogicalKeyboardKey.escape) {
            _restorePath();
            return KeyEventResult.handled;
          }
          return KeyEventResult.ignored;
        },
        child: TextField(
          key: ValueKey(widget.label),
          controller: _controller,
          maxLines: 1,
          autocorrect: false,
          enableSuggestions: false,
          textInputAction: TextInputAction.go,
          onSubmitted: widget.onNavigate,
          style: const TextStyle(fontFamily: 'monospace', fontSize: 11),
          decoration: InputDecoration(
            hintText: 'Enter a folder path and press Enter',
            isDense: true,
            contentPadding: const EdgeInsets.symmetric(
              horizontal: 9,
              vertical: 9,
            ),
            filled: true,
            fillColor: colors.surfaceContainerHighest.withValues(alpha: .55),
            border: OutlineInputBorder(borderRadius: BorderRadius.circular(7)),
            enabledBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(7),
              borderSide: BorderSide(color: colors.outlineVariant),
            ),
          ),
        ),
      ),
    );
  }
}

class _FileRow extends StatelessWidget {
  const _FileRow({
    required this.selected,
    required this.icon,
    required this.name,
    required this.detail,
  });

  final bool selected;
  final IconData icon;
  final String name;
  final String detail;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Container(
      color: selected ? colors.primaryContainer.withValues(alpha: .6) : null,
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      child: Row(
        children: [
          Icon(
            icon,
            size: 18,
            color: icon == Icons.folder_rounded
                ? const Color(0xFFE8AE42)
                : colors.onSurfaceVariant,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 12),
            ),
          ),
          Text(
            detail,
            style: TextStyle(color: colors.onSurfaceVariant, fontSize: 10),
          ),
        ],
      ),
    );
  }
}

String _baseName(String path) {
  final normalized = path.replaceAll('\\', '/');
  return normalized.substring(normalized.lastIndexOf('/') + 1);
}

String _localSize(FileSystemEntity entry) {
  try {
    return _formatSize((entry as File).lengthSync());
  } on Object {
    return '';
  }
}

String _formatSize(int? bytes) {
  if (bytes == null) return '';
  if (bytes < 1024) return '$bytes B';
  if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
  if (bytes < 1024 * 1024 * 1024) {
    return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
  }
  return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(1)} GB';
}

String _joinRemote(String parent, String name) {
  if (parent == '/') return '/$name';
  if (parent == '.') return './$name';
  return parent.endsWith('/') ? '$parent$name' : '$parent/$name';
}

String _remoteParent(String path) {
  if (path == '.' || path == '/') return path;
  final normalized = path.endsWith('/')
      ? path.substring(0, path.length - 1)
      : path;
  final slash = normalized.lastIndexOf('/');
  if (slash < 0) return '.';
  if (slash == 0) return '/';
  return normalized.substring(0, slash);
}
