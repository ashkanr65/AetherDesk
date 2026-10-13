import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

import '../assistant/workspace_tools.dart';
import 'tool_registry.dart';

typedef ToolRunnerFactory =
    WorkspaceTools Function(
      Directory directory,
      List<AssistantRemoteHost> Function() hosts,
    );

/// Reusable tools stay available without a provider account or an AI session.
class ToolsPanel extends StatefulWidget {
  const ToolsPanel({
    super.key,
    required this.registry,
    required this.remoteHosts,
    this.runnerFactory,
  });
  final ToolRegistry registry;
  final List<AssistantRemoteHost> Function() remoteHosts;
  final ToolRunnerFactory? runnerFactory;
  @override
  State<ToolsPanel> createState() => ToolsPanelState();
}

class ToolsPanelState extends State<ToolsPanel> {
  WorkspaceTools? _runner;
  String? _running;
  String? _error;
  String? _output;
  bool _loading = true;
  Future<void>? _closing;
  Future<void> shutdown() => _closing ??= _shutdown();
  Future<void> _shutdown() async {
    await _runner?.close();
  }

  @override
  void initState() {
    super.initState();
    widget.registry.addListener(_changed);
    unawaited(_reload());
  }

  @override
  void didUpdateWidget(ToolsPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.registry != widget.registry) {
      oldWidget.registry.removeListener(_changed);
      widget.registry.addListener(_changed);
      unawaited(_reload());
    }
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  Future<void> _reload() async {
    try {
      await widget.registry.reload();
    } catch (error) {
      if (mounted) setState(() => _error = error.toString());
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  void dispose() {
    widget.registry.removeListener(_changed);
    unawaited(shutdown().catchError((Object _) {}));
    super.dispose();
  }

  Future<void> _inspect(UserTool tool) => showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(tool.title),
      content: SizedBox(
        width: 640,
        child: SingleChildScrollView(
          child: SelectableText(
            const JsonEncoder.withIndent('  ').convert(tool.toJson()),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Done'),
        ),
      ],
    ),
  );

  Future<void> _remove(UserTool tool) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove ${tool.title}?'),
        content: const Text('This removes the saved tool from the Tools menu.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    try {
      await widget.registry.remove(tool.id, expectedRevision: tool.revision);
    } catch (error) {
      if (mounted) setState(() => _error = error.toString());
    }
  }

  Future<void> _import() async {
    try {
      final selected = await openFile(
        acceptedTypeGroups: [
          const XTypeGroup(label: 'Tool manifest', extensions: ['json']),
        ],
      );
      if (selected == null) return;
      if (await selected.length() > ToolRegistry.maxManifestBytes)
        throw const FormatException('Tool manifest exceeds 64 KiB.');
      final decoded = jsonDecode(await selected.readAsString());
      if (decoded is! Map<String, dynamic>)
        throw const FormatException('Tool manifest must be an object.');
      final tool = UserTool.fromJson(decoded);
      await widget.registry.reload();
      String revision = 'new';
      for (final existing in widget.registry.tools) {
        if (existing.id == tool.id) revision = existing.revision;
      }
      if (!mounted) return;
      final approved = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(
            '${revision == 'new' ? 'Install' : 'Update'} ${tool.title}?',
          ),
          content: SizedBox(
            width: 640,
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text(
                    'Review the tool below. Installing saves it; running it is a separate action.',
                  ),
                  const SizedBox(height: 12),
                  SelectableText(
                    const JsonEncoder.withIndent('  ').convert(tool.toJson()),
                  ),
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Install tool'),
            ),
          ],
        ),
      );
      if (approved == true)
        await widget.registry.install(decoded, expectedRevision: revision);
    } catch (error) {
      if (mounted) setState(() => _error = error.toString());
    }
  }

  Future<void> _run(UserTool tool) async {
    if (_running != null || _closing != null) return;
    final request = await showDialog<_RunRequest>(
      context: context,
      builder: (context) =>
          _RunToolDialog(tool: tool, hosts: widget.remoteHosts()),
    );
    if (request == null || !mounted || _running != null || _closing != null)
      return;
    setState(() {
      _running = tool.title;
      _output = null;
      _error = null;
    });
    WorkspaceTools? runner;
    try {
      final selected = request.host;
      List<AssistantRemoteHost> hosts() {
        if (selected == null) return [];
        if (!widget.remoteHosts().any(
          (host) =>
              host.id == selected.id && host.identity == selected.identity,
        ))
          return [];
        return [
          AssistantRemoteHost(
            id: selected.id,
            identity: selected.identity,
            label: selected.label,
            directory: request.directory,
          ),
        ];
      }

      final local = selected == null
          ? Directory(request.directory)
          : Directory.systemTemp;
      runner =
          widget.runnerFactory?.call(local, hosts) ??
          WorkspaceTools(
            localDirectory: local,
            remoteHosts: hosts,
            registry: widget.registry,
          );
      _runner = runner;
      await runner.setTarget(
        localDirectory: local,
        remoteSessionId: selected?.id,
      );
      final result = await runner.executeTool(
        tool.id,
        expectedRevision: tool.revision,
        arguments: request.arguments,
      );
      if (mounted)
        setState(
          () => _output = const JsonEncoder.withIndent('  ').convert(result),
        );
    } catch (error) {
      if (mounted) setState(() => _error = error.toString());
    } finally {
      try {
        await runner?.close();
      } catch (error) {
        if (mounted) setState(() => _error = error.toString());
      }
      _runner = null;
      if (mounted) setState(() => _running = null);
    }
  }

  Future<void> _stop() async {
    try {
      await _runner?.cancelPending();
    } catch (error) {
      if (mounted) setState(() => _error = error.toString());
    }
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(12, 8, 4, 8),
        child: Row(
          children: [
            Text('My tools', style: Theme.of(context).textTheme.titleMedium),
            const Spacer(),
            IconButton(
              tooltip: 'Import tool',
              onPressed: _running == null ? _import : null,
              icon: const Icon(Icons.file_open_outlined),
            ),
            IconButton(
              tooltip: 'Refresh tools',
              onPressed: _reload,
              icon: const Icon(Icons.refresh),
            ),
          ],
        ),
      ),
      if (_loading) const LinearProgressIndicator(),
      Expanded(
        child: ListView(
          padding: const EdgeInsets.all(12),
          children: [
            if (widget.registry.tools.isEmpty)
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 12),
                child: Text(
                  'Ask AI Assistant to create a reusable tool for Windows or a connected SSH host. After you approve its installation, it appears here. You can also import a tool manifest.',
                ),
              ),
            if (widget.registry.invalidFiles.isNotEmpty)
              Text(
                'Some tool manifests could not be loaded: ${widget.registry.invalidFiles.join(', ')}',
              ),
            for (final tool in widget.registry.tools)
              Card(
                child: Padding(
                  padding: const EdgeInsets.all(12),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        tool.title,
                        style: Theme.of(context).textTheme.titleSmall,
                      ),
                      const SizedBox(height: 4),
                      Text(tool.description),
                      const SizedBox(height: 6),
                      Text(
                        '${tool.target == 'windows' ? 'Windows' : 'SSH host'} · ${tool.version}',
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                      Wrap(
                        spacing: 4,
                        children: [
                          TextButton.icon(
                            onPressed: _running == null
                                ? () => _run(tool)
                                : null,
                            icon: const Icon(Icons.play_arrow),
                            label: const Text('Run'),
                          ),
                          TextButton(
                            onPressed: () => _inspect(tool),
                            child: const Text('Inspect'),
                          ),
                          IconButton(
                            tooltip: 'Remove ${tool.title}',
                            onPressed: _running == null
                                ? () => _remove(tool)
                                : null,
                            icon: const Icon(Icons.delete_outline),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            if (_running != null)
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const LinearProgressIndicator(),
                  Text('Running $_running…'),
                  TextButton.icon(
                    onPressed: _stop,
                    icon: const Icon(Icons.stop),
                    label: const Text('Stop'),
                  ),
                ],
              ),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: SelectableText(
                  _error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ),
            if (_output != null)
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text('Last result'),
                  const SizedBox(height: 8),
                  SelectableText(_output!),
                ],
              ),
          ],
        ),
      ),
    ],
  );
}

class _RunRequest {
  const _RunRequest(this.directory, this.host, this.arguments);
  final String directory;
  final AssistantRemoteHost? host;
  final Map<String, String> arguments;
}

class _RunToolDialog extends StatefulWidget {
  const _RunToolDialog({required this.tool, required this.hosts});
  final UserTool tool;
  final List<AssistantRemoteHost> hosts;
  @override
  State<_RunToolDialog> createState() => _RunToolDialogState();
}

class _RunToolDialogState extends State<_RunToolDialog> {
  late final TextEditingController _directory;
  late final Map<String, TextEditingController> _arguments;
  AssistantRemoteHost? _host;
  String? _error;
  String? _reviewCommand;
  @override
  void initState() {
    super.initState();
    if (widget.tool.target == 'ssh' && widget.hosts.isNotEmpty)
      _host = widget.hosts.first;
    _directory = TextEditingController(
      text: widget.tool.target == 'ssh'
          ? _host?.directory ?? '.'
          : Platform.environment['USERPROFILE'] ?? Directory.current.path,
    );
    _arguments = {
      for (final parameter in widget.tool.parameters)
        parameter.name: TextEditingController(
          text: parameter.defaultValue ?? '',
        ),
    };
  }

  @override
  void dispose() {
    _directory.dispose();
    for (final controller in _arguments.values) {
      controller.dispose();
    }
    super.dispose();
  }

  void _review() {
    try {
      if (_directory.text.trim().isEmpty || _directory.text.contains('\u0000'))
        throw const FormatException('Choose a working folder.');
      if (widget.tool.target == 'ssh' && _host == null)
        throw StateError('Connect an SSH host before running this tool.');
      final command = widget.tool.command({
        for (final entry in _arguments.entries) entry.key: entry.value.text,
      });
      setState(() {
        _reviewCommand = command;
        _error = null;
      });
    } catch (error) {
      setState(() => _error = error.toString());
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(
      '${_reviewCommand == null ? 'Run' : 'Review'} ${widget.tool.title}',
    ),
    content: SizedBox(
      width: 620,
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (_reviewCommand == null) ...[
              if (widget.tool.target == 'ssh')
                DropdownButtonFormField<String>(
                  initialValue: _host?.id,
                  decoration: const InputDecoration(labelText: 'SSH host'),
                  items: [
                    for (final host in widget.hosts)
                      DropdownMenuItem(value: host.id, child: Text(host.label)),
                  ],
                  onChanged: (id) => setState(() {
                    _host = widget.hosts.firstWhere((host) => host.id == id);
                    _directory.text = _host!.directory;
                  }),
                ),
              Row(
                children: [
                  Expanded(
                    child: TextField(
                      controller: _directory,
                      decoration: const InputDecoration(
                        labelText: 'Working folder',
                      ),
                    ),
                  ),
                  if (widget.tool.target == 'windows')
                    IconButton(
                      tooltip: 'Choose folder',
                      icon: const Icon(Icons.folder_open),
                      onPressed: () async {
                        final path = await getDirectoryPath();
                        if (path != null && mounted)
                          setState(() => _directory.text = path);
                      },
                    ),
                ],
              ),
              for (final parameter in widget.tool.parameters)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: TextField(
                    controller: _arguments[parameter.name],
                    decoration: InputDecoration(
                      labelText: parameter.label,
                      helperText: parameter.required ? 'Required' : 'Optional',
                    ),
                  ),
                ),
            ] else ...[
              Text('Target: ${_host?.label ?? 'This Windows computer'}'),
              Text('Folder: ${_directory.text}'),
              const SizedBox(height: 12),
              const Text(
                'This script runs with your normal account access. Review it before running.',
              ),
              const SizedBox(height: 12),
              SelectableText(_reviewCommand!),
            ],
            if (_error != null)
              Text(
                _error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
          ],
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      if (_reviewCommand != null)
        TextButton(
          onPressed: () => setState(() => _reviewCommand = null),
          child: const Text('Back'),
        ),
      FilledButton(
        onPressed: _reviewCommand == null
            ? _review
            : () => Navigator.pop(
                context,
                _RunRequest(_directory.text, _host, {
                  for (final entry in _arguments.entries)
                    entry.key: entry.value.text,
                }),
              ),
        child: Text(_reviewCommand == null ? 'Review script' : 'Run tool'),
      ),
    ],
  );
}
