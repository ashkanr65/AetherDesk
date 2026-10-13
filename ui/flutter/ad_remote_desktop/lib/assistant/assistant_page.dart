import 'dart:async';
import 'dart:convert';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'assistant_controller.dart';

/// Native chat surface. The parent owns the controller and its process lifetime.
class AssistantPage extends StatefulWidget {
  const AssistantPage({super.key, required this.controller});

  final AssistantController controller;

  @override
  State<AssistantPage> createState() => _AssistantPageState();
}

class _AssistantPageState extends State<AssistantPage> {
  final _draft = TextEditingController();
  final _scroll = ScrollController();
  String? _actionError;
  String _lastContent = '';
  bool _changingWorkspace = false;
  bool _capturing = false;
  Map<String, dynamic>? _capture;

  AssistantController get controller => widget.controller;

  @override
  void dispose() {
    _draft.dispose();
    _scroll.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function() action) async {
    setState(() => _actionError = null);
    try {
      await action();
    } on Object catch (error) {
      if (mounted) setState(() => _actionError = '$error');
    }
  }

  Future<void> _send() async {
    final text = _draft.text.trim();
    if (text.isEmpty || !_canSend) return;
    final capture = _capture;
    _draft.clear();
    setState(() => _capture = null);
    await _run(() async {
      try {
        await controller.send(text, capture: capture);
      } on Object {
        if (mounted && _draft.text.isEmpty) {
          _draft.text = text;
          setState(() => _capture = capture);
        }
        rethrow;
      }
    });
  }

  KeyEventResult _composerKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent ||
        (event.logicalKey != LogicalKeyboardKey.enter &&
            event.logicalKey != LogicalKeyboardKey.numpadEnter)) {
      return KeyEventResult.ignored;
    }
    final composing = _draft.value.composing;
    if (composing.isValid && !composing.isCollapsed)
      return KeyEventResult.ignored;
    if (HardwareKeyboard.instance.isAltPressed ||
        HardwareKeyboard.instance.isMetaPressed ||
        HardwareKeyboard.instance.isShiftPressed)
      return KeyEventResult.ignored;
    if (HardwareKeyboard.instance.isControlPressed) {
      final value = _draft.value;
      final selection = value.selection;
      final start = selection.isValid ? selection.start : value.text.length;
      final end = selection.isValid ? selection.end : value.text.length;
      _draft.value = TextEditingValue(
        text: value.text.replaceRange(start, end, '\n'),
        selection: TextSelection.collapsed(offset: start + 1),
      );
    } else {
      unawaited(_send());
    }
    return KeyEventResult.handled;
  }

  bool get _canSend =>
      controller.phase == AssistantPhase.ready &&
      controller.selectedModel != null &&
      !controller.isBusy &&
      !_changingWorkspace &&
      !_capturing;

  void _followMessages() {
    final content = controller.messages
        .map(
          (m) =>
              '${m.id}:${m.text.length}:${m.tools.map((t) => t.status).join()}',
        )
        .join();
    if (content == _lastContent) return;
    _lastContent = content;
    final follow =
        !_scroll.hasClients ||
        _scroll.position.maxScrollExtent - _scroll.offset < 120;
    if (!follow) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
  }

  Future<void> _chooseWorkspace() async {
    final selection = await showDialog<_WorkspaceSelection>(
      context: context,
      builder: (context) => _WorkspaceDialog(controller: controller),
    );
    if (selection == null || !mounted) return;
    setState(() => _changingWorkspace = true);
    await _run(
      () => controller.chooseWorkspace(
        selection.path,
        remoteSessionId: selection.remoteSessionId,
      ),
    );
    if (mounted) setState(() => _changingWorkspace = false);
  }

  Future<void> _connectProvider() async {
    final connection = await showDialog<_ProviderConnection>(
      context: context,
      builder: (context) => _ProviderDialog(
        providers: controller.providers,
        settingsFor: controller.providerSettings,
        selectedProviderId: controller.selectedModel?.providerId,
      ),
    );
    if (connection == null || !mounted) return;
    if (connection.useExisting) {
      final provider = controller.providers
          .where(
            (provider) => provider.id == connection.id && provider.connected,
          )
          .firstOrNull;
      if (provider != null) await _chooseModel(provider.models);
      return;
    }
    if (connection.remove) {
      await _run(() => controller.removeProvider(connection.id));
      return;
    }
    await _run(
      () => controller.connectProvider(
        connection.id,
        connection.key,
        baseUrl: connection.baseUrl,
        modelId: connection.modelId,
      ),
    );
  }

  Future<void> _selectHistory(String value) async {
    if (!value.startsWith('delete:') && value != 'clear-history') {
      await _run(
        value.isEmpty
            ? controller.newConversation
            : () => controller.selectConversation(value),
      );
      return;
    }
    final id = value.startsWith('delete:') ? value.substring(7) : null;
    final title = id == null
        ? 'Clear all AI history?'
        : 'Delete this conversation?';
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title),
        content: Text(
          id == null
              ? 'Delete all saved AI conversations and attached screens from AetherDesk? Your files, tools and provider connections will be kept.'
              : 'Delete this saved conversation and its attached screens from AetherDesk? Your files and tools will be kept.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(id == null ? 'Clear history' : 'Delete conversation'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    final current = id == null || id == controller.currentSessionId;
    await _run(() async {
      if (id == null) {
        await controller.clearHistory();
      } else {
        await controller.deleteConversation(id);
      }
      if (mounted && current)
        setState(() {
          _draft.clear();
          _capture = null;
        });
    });
  }

  Future<void> _chooseModel(List<AssistantModel> models) async {
    final id = await showDialog<String>(
      context: context,
      builder: (context) => _ChoiceDialog(
        title: 'Choose model',
        searchHint: 'Search models',
        emptyMessage: 'Connect a provider to see its models.',
        selectedId: controller.selectedModel?.key,
        choices: [
          for (final model in models)
            _Choice(
              id: model.key,
              label: model.name,
              detail:
                  '${controller.providers.firstWhere((p) => p.id == model.providerId).name} · ${model.id}',
              keywords: model.providerId,
            ),
        ],
      ),
    );
    if (id == null || !mounted) return;
    await _run(() async {
      controller.selectModel(models.firstWhere((model) => model.key == id));
    });
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: controller,
    builder: (context, _) {
      _followMessages();
      final colors = Theme.of(context).colorScheme;
      final ready = controller.phase == AssistantPhase.ready;
      final models = [
        for (final provider in controller.providers)
          if (provider.connected) ...provider.models,
      ];
      final selected = controller.selectedModel;
      final selectedKey = selected == null
          ? null
          : '${selected.providerId}/${selected.id}';
      final error = _actionError ?? controller.error;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Material(
            color: colors.surfaceContainerLow,
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Wrap(
                spacing: 12,
                runSpacing: 12,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  Row(
                    children: [
                      const Expanded(
                        child: Text(
                          'AI Assistant',
                          style: TextStyle(
                            fontSize: 18,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ),
                      PopupMenuButton<String>(
                        tooltip: 'Conversation history',
                        enabled:
                            ready && !controller.isBusy && !_changingWorkspace,
                        icon: const Icon(Icons.history),
                        onSelected: _selectHistory,
                        itemBuilder: (context) => [
                          const PopupMenuItem(
                            value: '',
                            child: Text('New conversation'),
                          ),
                          if (controller.conversations.isNotEmpty)
                            const PopupMenuDivider(),
                          for (final conversation in controller.conversations)
                            PopupMenuItem(
                              value: conversation.id,
                              child: Row(
                                children: [
                                  Expanded(
                                    child: Text(
                                      conversation.title,
                                      maxLines: 2,
                                      overflow: TextOverflow.ellipsis,
                                    ),
                                  ),
                                  IconButton(
                                    tooltip: 'Delete ${conversation.title}',
                                    key: ValueKey(
                                      'delete-chat-${conversation.id}',
                                    ),
                                    icon: const Icon(
                                      Icons.delete_outline,
                                      size: 18,
                                    ),
                                    onPressed: () => Navigator.pop(
                                      context,
                                      'delete:${conversation.id}',
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          if (controller.conversations.isNotEmpty) ...[
                            const PopupMenuDivider(),
                            const PopupMenuItem(
                              value: 'clear-history',
                              child: Text('Clear all history'),
                            ),
                          ],
                        ],
                      ),
                    ],
                  ),
                  OutlinedButton.icon(
                    key: const Key('assistant-workspace'),
                    onPressed:
                        ready && !controller.isBusy && !_changingWorkspace
                        ? _chooseWorkspace
                        : null,
                    icon: Icon(
                      controller.remoteSessionId == null
                          ? Icons.folder_outlined
                          : Icons.dns_outlined,
                    ),
                    label: const Text('Workspace'),
                  ),
                  SizedBox(
                    width: 230,
                    child: _SelectionField(
                      key: const Key('assistant-model'),
                      label: 'Model',
                      value: models.any((model) => model.key == selectedKey)
                          ? selected?.name
                          : null,
                      onPressed:
                          ready && !controller.isBusy && !_changingWorkspace
                          ? () => _chooseModel(models)
                          : null,
                    ),
                  ),
                  IconButton(
                    tooltip: 'Manage providers',
                    onPressed: ready && !controller.isBusy
                        ? _connectProvider
                        : null,
                    icon: const Icon(Icons.key_outlined, size: 18),
                  ),
                ],
              ),
            ),
          ),
          if (ready)
            Padding(
              padding: const EdgeInsets.fromLTRB(18, 10, 18, 4),
              child: Text(
                '${_workspaceLabel()} · ${_workspacePath()}',
                style: TextStyle(color: colors.onSurfaceVariant),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          if (ready && controller.allActionsAllowed)
            TextButton.icon(
              onPressed: () => _run(controller.revokeApprovals),
              icon: const Icon(Icons.lock_open),
              label: const Text('All actions allowed · Ask again'),
            ),
          if (controller.captureConnection != null)
            TextButton.icon(
              key: const Key('assistant-capture'),
              onPressed: !_canSend
                  ? null
                  : () => _run(() async {
                      setState(() => _capturing = true);
                      try {
                        final capture = await controller.captureConnection!();
                        if (mounted) setState(() => _capture = capture);
                      } finally {
                        if (mounted) setState(() => _capturing = false);
                      }
                    }),
              icon: const Icon(Icons.add_photo_alternate_outlined),
              label: Text(
                _capturing ? 'Capturing…' : 'Attach connection screen',
              ),
            ),
          if (_capture != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 12),
              child: Column(
                children: [
                  _ChatScreenImage(url: _capture!['url'] as String, height: 90),
                  Text(
                    'Screen: ${_capture!['label']}. Sent with your next message; choose a vision model.',
                  ),
                  TextButton(
                    onPressed: () => setState(() => _capture = null),
                    child: const Text('Remove screen'),
                  ),
                ],
              ),
            ),
          if (error != null)
            Padding(
              padding: const EdgeInsets.all(12),
              child: Material(
                color: colors.errorContainer,
                borderRadius: BorderRadius.circular(10),
                child: Padding(
                  padding: const EdgeInsets.all(12),
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxHeight: 96),
                    child: SingleChildScrollView(
                      child: SelectableText(
                        error,
                        style: TextStyle(color: colors.onErrorContainer),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          Expanded(child: _body()),
          if (ready) _composer(),
        ],
      );
    },
  );

  String _workspaceLabel() {
    final id = controller.remoteSessionId;
    if (id == null) return 'This computer';
    for (final host in controller.remoteHosts) {
      if (host.id == id) return host.label;
    }
    return 'SSH host (disconnected)';
  }

  String _workspacePath() {
    if (controller.remoteSessionId == null) return controller.workspacePath;
    for (final host in controller.remoteHosts) {
      if (host.id == controller.remoteSessionId) return host.directory;
    }
    return 'Reconnect to resume this workspace';
  }

  Widget _body() {
    switch (controller.phase) {
      case AssistantPhase.starting:
        return const Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              CircularProgressIndicator(),
              SizedBox(height: 16),
              Text('Starting your assistant…'),
            ],
          ),
        );
      case AssistantPhase.stopped:
      case AssistantPhase.failed:
        return Center(
          child: FilledButton.icon(
            onPressed: () => _run(controller.start),
            icon: const Icon(Icons.refresh),
            label: Text(
              controller.phase == AssistantPhase.failed
                  ? 'Try again'
                  : 'Start assistant',
            ),
          ),
        );
      case AssistantPhase.ready:
        if (_changingWorkspace) {
          return const Center(child: CircularProgressIndicator());
        }
        return ListView(
          key: const Key('assistant-messages'),
          controller: _scroll,
          padding: const EdgeInsets.all(20),
          children: [
            if (controller.messages.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 48),
                child: Column(
                  children: [
                    const Icon(Icons.auto_awesome_outlined, size: 40),
                    const SizedBox(height: 16),
                    const Text(
                      'What would you like to work on?',
                      style: TextStyle(fontSize: 22),
                    ),
                    const SizedBox(height: 10),
                    Text(
                      controller.selectedModel == null
                          ? 'Connect an AI provider and select a model to begin.'
                          : 'Ask about files, make changes, or run commands in your selected workspace.',
                      textAlign: TextAlign.center,
                    ),
                  ],
                ),
              ),
            for (final message in controller.messages)
              _MessageCard(key: ValueKey(message.id), message: message),
            for (final approval in controller.approvals)
              _ApprovalCard(
                key: ValueKey('approval-${approval.id}'),
                approval: approval,
                onReply: (allow) => _run(
                  () => controller.replyPermission(
                    approval.id,
                    allow != 'reject',
                    scope: allow == 'reject' ? 'once' : allow,
                  ),
                ),
              ),
            for (final question in controller.questions)
              _QuestionCard(
                key: ValueKey(question.id),
                question: question,
                onReply: (answers) =>
                    _run(() => controller.replyQuestion(question.id, answers)),
                onReject: () =>
                    _run(() => controller.rejectQuestion(question.id)),
              ),
            if (controller.isBusy)
              Padding(
                padding: const EdgeInsets.all(12),
                child: Row(
                  children: [
                    const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    ),
                    const SizedBox(width: 10),
                    Flexible(
                      child: Text(
                        controller.progress ?? 'Assistant is working…',
                      ),
                    ),
                  ],
                ),
              ),
          ],
        );
    }
  }

  Widget _composer() => Padding(
    padding: const EdgeInsets.fromLTRB(18, 8, 18, 16),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        Expanded(
          child: Focus(
            onKeyEvent: _composerKey,
            child: TextField(
              key: const Key('assistant-prompt'),
              controller: _draft,
              minLines: 2,
              maxLines: 6,
              enabled: !_changingWorkspace,
              decoration: const InputDecoration(
                hintText: 'Message… (Enter sends, Ctrl+Enter adds a line)',
              ),
            ),
          ),
        ),
        const SizedBox(width: 10),
        if (controller.isBusy)
          IconButton.filled(
            key: const Key('assistant-stop'),
            tooltip: 'Stop',
            onPressed: () => _run(controller.abort),
            icon: const Icon(Icons.stop_rounded),
          )
        else
          IconButton.filled(
            key: const Key('assistant-send'),
            tooltip: 'Send (Enter)',
            onPressed: _canSend ? _send : null,
            icon: const Icon(Icons.arrow_upward_rounded),
          ),
      ],
    ),
  );
}

class AssistantHistory extends StatelessWidget {
  const AssistantHistory({super.key, required this.controller});

  final AssistantController controller;

  Future<void> _run(
    BuildContext context,
    Future<void> Function() action,
  ) async {
    try {
      await action();
    } on Object catch (error) {
      if (context.mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not open conversation: $error')),
        );
      }
    }
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: controller,
    builder: (context, _) => Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Padding(
          padding: EdgeInsets.fromLTRB(16, 18, 16, 12),
          child: Text(
            'AI Assistant',
            style: TextStyle(fontSize: 17, fontWeight: FontWeight.w700),
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: OutlinedButton.icon(
            onPressed:
                controller.phase == AssistantPhase.ready && !controller.isBusy
                ? () => _run(context, controller.newConversation)
                : null,
            icon: const Icon(Icons.add),
            label: const Text('New conversation'),
          ),
        ),
        const SizedBox(height: 12),
        Expanded(
          child: ListView(
            children: [
              for (final conversation in controller.conversations)
                ListTile(
                  leading: const Icon(Icons.chat_bubble_outline, size: 18),
                  title: Text(
                    conversation.title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                  selected: conversation.id == controller.currentSessionId,
                  enabled:
                      !controller.isBusy &&
                      controller.phase == AssistantPhase.ready,
                  onTap: () => _run(
                    context,
                    () => controller.selectConversation(conversation.id),
                  ),
                ),
            ],
          ),
        ),
        const Padding(
          padding: EdgeInsets.all(16),
          child: Text('Powered by LangGraph', style: TextStyle(fontSize: 12)),
        ),
      ],
    ),
  );
}

class _MessageCard extends StatelessWidget {
  const _MessageCard({super.key, required this.message});
  final AssistantMessage message;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final user = message.role == 'user';
    return Card(
      elevation: 0,
      color: user ? colors.primaryContainer : colors.surfaceContainerLow,
      margin: const EdgeInsets.only(bottom: 14),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              user ? 'You' : 'Assistant',
              style: const TextStyle(fontWeight: FontWeight.w700),
            ),
            if (message.text.isNotEmpty) ...[
              const SizedBox(height: 8),
              SelectableText(message.text),
            ],
            for (final url in message.images)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: _ChatScreenImage(url: url, height: 140),
              ),
            for (final tool in message.tools)
              ExpansionTile(
                tilePadding: EdgeInsets.zero,
                title: Text(tool.name),
                subtitle: Text(tool.status),
                children: [
                  Align(
                    alignment: Alignment.centerLeft,
                    child: SelectableText(
                      '${_detail(tool.input)}\n${_detail(tool.output)}',
                      style: const TextStyle(
                        fontFamily: 'monospace',
                        fontSize: 12,
                      ),
                    ),
                  ),
                ],
              ),
            if (message.error != null) ...[
              const SizedBox(height: 8),
              SelectableText(
                message.error!,
                style: TextStyle(color: colors.error),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

// Keep decoded bytes stable across polling and streamed-text rebuilds.
// MemoryImage identifies its cache entry by the byte buffer's identity.
class _ChatScreenImage extends StatefulWidget {
  const _ChatScreenImage({required this.url, required this.height});
  final String url;
  final double height;
  @override
  State<_ChatScreenImage> createState() => _ChatScreenImageState();
}

class _ChatScreenImageState extends State<_ChatScreenImage> {
  late Uint8List _bytes;
  @override
  void initState() {
    super.initState();
    _bytes = base64Decode(widget.url.split(',').last);
  }

  @override
  void didUpdateWidget(_ChatScreenImage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.url != widget.url) {
      _bytes = base64Decode(widget.url.split(',').last);
    }
  }

  @override
  Widget build(BuildContext context) => Image.memory(
    _bytes,
    height: widget.height,
    gaplessPlayback: true,
    errorBuilder: (context, error, stack) =>
        const Text('Screen preview unavailable.'),
  );
}

String _detail(Object? value) {
  if (value == null) return '';
  if (value is String) return value;
  return const JsonEncoder.withIndent('  ').convert(value);
}

class _ApprovalCard extends StatefulWidget {
  const _ApprovalCard({
    super.key,
    required this.approval,
    required this.onReply,
  });
  final AssistantApproval approval;
  final Future<void> Function(String) onReply;

  @override
  State<_ApprovalCard> createState() => _ApprovalCardState();
}

class _ApprovalCardState extends State<_ApprovalCard> {
  bool _submitting = false;

  Future<void> _reply(String allow) async {
    setState(() => _submitting = true);
    try {
      await widget.onReply(allow);
    } finally {
      if (mounted) setState(() => _submitting = false);
    }
  }

  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Allow ${widget.approval.permission}?',
            style: const TextStyle(fontWeight: FontWeight.w700),
          ),
          const SizedBox(height: 8),
          for (final pattern in widget.approval.patterns)
            SelectableText(pattern),
          if (widget.approval.metadata.isNotEmpty)
            ExpansionTile(
              title: const Text('Action details'),
              tilePadding: EdgeInsets.zero,
              children: [SelectableText(_detail(widget.approval.metadata))],
            ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            children: [
              FilledButton(
                onPressed: _submitting ? null : () => _reply('message'),
                child: const Text('Allow this message'),
              ),
              OutlinedButton(
                onPressed: _submitting ? null : () => _reply('session'),
                child: const Text('Allow all in this chat'),
              ),
              TextButton(
                onPressed: _submitting ? null : () => _reply('once'),
                child: const Text('Only this action'),
              ),
              OutlinedButton(
                onPressed: _submitting ? null : () => _reply('reject'),
                child: const Text('Deny'),
              ),
            ],
          ),
        ],
      ),
    ),
  );
}

class _QuestionCard extends StatefulWidget {
  const _QuestionCard({
    super.key,
    required this.question,
    required this.onReply,
    required this.onReject,
  });
  final AssistantQuestion question;
  final Future<void> Function(List<List<String>>) onReply;
  final Future<void> Function() onReject;

  @override
  State<_QuestionCard> createState() => _QuestionCardState();
}

class _QuestionCardState extends State<_QuestionCard> {
  late final _selected = [
    for (final _ in widget.question.questions) <String>{},
  ];
  late final _custom = [
    for (final _ in widget.question.questions) TextEditingController(),
  ];
  bool _submitting = false;

  @override
  void dispose() {
    for (final field in _custom) {
      field.dispose();
    }
    super.dispose();
  }

  List<List<String>> get _answers => [
    for (var i = 0; i < _selected.length; i++)
      [
        ..._selected[i],
        if (_custom[i].text.trim().isNotEmpty) _custom[i].text.trim(),
      ],
  ];

  Future<void> _submit(Future<void> Function() action) async {
    setState(() => _submitting = true);
    try {
      await action();
    } finally {
      if (mounted) setState(() => _submitting = false);
    }
  }

  @override
  Widget build(BuildContext context) => Card(
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          for (var i = 0; i < widget.question.questions.length; i++) ...[
            Text(
              '${widget.question.questions[i]['question'] ?? 'Your choice'}',
              style: const TextStyle(fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 6,
              children: [
                for (final option
                    in (widget.question.questions[i]['options'] as List? ??
                        const []))
                  Tooltip(
                    message: '${option['description'] ?? ''}',
                    child: FilterChip(
                      label: Text('${option['label']}'),
                      selected: _selected[i].contains('${option['label']}'),
                      onSelected: _submitting
                          ? null
                          : (selected) => setState(() {
                              if (widget.question.questions[i]['multiple'] !=
                                  true) {
                                _selected[i].clear();
                                _custom[i].clear();
                              }
                              if (selected) {
                                _selected[i].add('${option['label']}');
                              } else {
                                _selected[i].remove('${option['label']}');
                              }
                            }),
                    ),
                  ),
              ],
            ),
            if (widget.question.questions[i]['custom'] != false)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: TextField(
                  controller: _custom[i],
                  enabled: !_submitting,
                  decoration: const InputDecoration(labelText: 'Your answer'),
                  onChanged: (_) => setState(() {
                    if (widget.question.questions[i]['multiple'] != true) {
                      _selected[i].clear();
                    }
                  }),
                ),
              ),
            const SizedBox(height: 16),
          ],
          Wrap(
            spacing: 8,
            children: [
              FilledButton(
                onPressed:
                    !_submitting &&
                        _answers.every((answer) => answer.isNotEmpty)
                    ? () => _submit(() => widget.onReply(_answers))
                    : null,
                child: const Text('Answer'),
              ),
              TextButton(
                onPressed: _submitting ? null : () => _submit(widget.onReject),
                child: const Text('Skip question'),
              ),
            ],
          ),
        ],
      ),
    ),
  );
}

class _WorkspaceSelection {
  const _WorkspaceSelection(this.path, this.remoteSessionId);
  final String path;
  final String? remoteSessionId;
}

class _WorkspaceDialog extends StatefulWidget {
  const _WorkspaceDialog({required this.controller});
  final AssistantController controller;
  @override
  State<_WorkspaceDialog> createState() => _WorkspaceDialogState();
}

class _WorkspaceDialogState extends State<_WorkspaceDialog> {
  late final TextEditingController _path;
  late final List<AssistantRemoteHost> _hosts;
  String? _remoteId;

  @override
  void initState() {
    super.initState();
    _hosts = widget.controller.remoteHosts;
    final previous = widget.controller.remoteSessionId;
    final connected = _hosts.any((host) => host.id == previous);
    _remoteId = connected ? previous : null;
    _path = TextEditingController(
      text: previous == null
          ? widget.controller.workspacePath
          : connected
          ? _hosts.firstWhere((host) => host.id == previous).directory
          : '',
    );
  }

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final hosts = _hosts;
    final current = hosts.any((host) => host.id == _remoteId)
        ? _remoteId
        : null;
    return AlertDialog(
      title: const Text('Choose workspace'),
      content: SizedBox(
        width: 520,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            DropdownButtonFormField<String>(
              key: const Key('assistant-source'),
              initialValue: current ?? '',
              decoration: const InputDecoration(labelText: 'Computer'),
              isExpanded: true,
              items: [
                const DropdownMenuItem(
                  value: '',
                  child: Text('This Windows computer'),
                ),
                for (final host in hosts)
                  DropdownMenuItem(
                    value: host.id,
                    child: Text(host.label, overflow: TextOverflow.ellipsis),
                  ),
              ],
              onChanged: (value) => setState(() {
                _remoteId = value == null || value.isEmpty ? null : value;
                _path.text = _remoteId == null
                    ? (widget.controller.remoteSessionId == null
                          ? widget.controller.workspacePath
                          : '')
                    : hosts
                          .firstWhere((host) => host.id == _remoteId)
                          .directory;
              }),
            ),
            const SizedBox(height: 14),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    key: const Key('assistant-workspace-path'),
                    controller: _path,
                    decoration: const InputDecoration(labelText: 'Folder'),
                    onChanged: (_) => setState(() {}),
                  ),
                ),
                if (_remoteId == null) ...[
                  const SizedBox(width: 8),
                  IconButton(
                    tooltip: 'Browse Windows folders',
                    icon: const Icon(Icons.folder_open),
                    onPressed: () async {
                      final selected = await getDirectoryPath();
                      if (selected != null && mounted)
                        setState(() => _path.text = selected);
                    },
                  ),
                ],
              ],
            ),
            const SizedBox(height: 12),
            const Text(
              'Choose the folder the assistant should work in. Changing workspace starts a new conversation.',
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _path.text.trim().isEmpty
              ? null
              : () => Navigator.pop(
                  context,
                  _WorkspaceSelection(_path.text.trim(), _remoteId),
                ),
          child: const Text('Use workspace'),
        ),
      ],
    );
  }
}

class _ProviderConnection {
  const _ProviderConnection(
    this.id,
    this.key,
    this.baseUrl,
    this.modelId, {
    this.remove = false,
    this.useExisting = false,
  });
  final bool remove, useExisting;
  final String id;
  final String key;
  final String baseUrl, modelId;
}

class _ProviderDialog extends StatefulWidget {
  const _ProviderDialog({
    required this.providers,
    required this.settingsFor,
    this.selectedProviderId,
  });
  final String? selectedProviderId;
  final Map<String, dynamic> Function(String id) settingsFor;
  final List<AssistantProvider> providers;
  @override
  State<_ProviderDialog> createState() => _ProviderDialogState();
}

class _ProviderDialogState extends State<_ProviderDialog> {
  final _key = TextEditingController();
  final _baseUrl = TextEditingController();
  final _modelId = TextEditingController();
  String? _providerId;
  AssistantProvider? get _provider =>
      widget.providers.where((p) => p.id == _providerId).firstOrNull;
  @override
  void dispose() {
    _key.clear();
    _key.dispose();
    _baseUrl.dispose();
    _modelId.dispose();
    super.dispose();
  }

  Future<void> _chooseProvider() async {
    final id = await showDialog<String>(
      context: context,
      builder: (context) => _ChoiceDialog(
        title: 'Choose provider',
        searchHint: 'Search providers',
        emptyMessage: 'No providers available.',
        selectedId: _providerId,
        choices: [
          for (final provider in widget.providers)
            _Choice(
              id: provider.id,
              label: provider.name,
              detail:
                  '${provider.connected ? 'Connected · ' : ''}${provider.id}',
            ),
        ],
      ),
    );
    if (id != null && mounted && id != _providerId) _editProvider(id);
  }

  void _editProvider(String id) => setState(() {
    _providerId = id;
    _key.clear();
    final saved = widget.settingsFor(id);
    _baseUrl.text = saved['baseURL'] as String? ?? _provider!.baseUrl;
    _modelId.text = saved['modelId'] as String? ?? '';
  });

  List<AssistantProvider> get _connected {
    final providers = widget.providers
        .where(
          (provider) =>
              provider.connected || widget.settingsFor(provider.id).isNotEmpty,
        )
        .toList();
    providers.sort(
      (a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()),
    );
    return providers;
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Manage AI providers'),
    content: SizedBox(
      width: 440,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Align(
              alignment: Alignment.centerLeft,
              child: Text(
                'Connected services',
                style: Theme.of(context).textTheme.titleSmall,
              ),
            ),
            const SizedBox(height: 8),
            if (_connected.isEmpty)
              const Padding(
                padding: EdgeInsets.only(bottom: 12),
                child: Text('No services connected yet.'),
              ),
            for (final provider in _connected)
              ListTile(
                key: ValueKey('assistant-connected-${provider.id}'),
                contentPadding: EdgeInsets.zero,
                selected: provider.id == widget.selectedProviderId,
                leading: Icon(
                  provider.id == widget.selectedProviderId
                      ? Icons.check_circle_outline
                      : Icons.cloud_done_outlined,
                ),
                title: Text(provider.name),
                subtitle: Text(
                  provider.models.isEmpty
                      ? 'No models available · Edit connection'
                      : 'Select a model from this service',
                ),
                onTap: provider.connected && provider.models.isNotEmpty
                    ? () => Navigator.pop(
                        context,
                        _ProviderConnection(
                          provider.id,
                          '',
                          '',
                          '',
                          useExisting: true,
                        ),
                      )
                    : () => _editProvider(provider.id),
                trailing: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    IconButton(
                      key: ValueKey('assistant-edit-service-${provider.id}'),
                      tooltip: 'Edit ${provider.name}',
                      icon: const Icon(Icons.edit_outlined),
                      onPressed: () => _editProvider(provider.id),
                    ),
                    IconButton(
                      key: ValueKey('assistant-remove-service-${provider.id}'),
                      tooltip: 'Remove ${provider.name}',
                      icon: const Icon(Icons.delete_outline),
                      onPressed: () => Navigator.pop(
                        context,
                        _ProviderConnection(
                          provider.id,
                          '',
                          '',
                          '',
                          remove: true,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            const Divider(),
            const SizedBox(height: 8),
            _SelectionField(
              key: const Key('assistant-provider'),
              label: 'Provider',
              value: widget.providers
                  .where((provider) => provider.id == _providerId)
                  .firstOrNull
                  ?.name,
              onPressed: _chooseProvider,
            ),
            const SizedBox(height: 16),
            TextField(
              controller: _key,
              obscureText: true,
              autocorrect: false,
              enableSuggestions: false,
              decoration: InputDecoration(
                labelText: _provider?.requiresApiKey == false
                    ? 'API key (optional)'
                    : 'API key',
              ),
              onChanged: (_) => setState(() {}),
            ),
            if (_provider != null) ...[
              const SizedBox(height: 16),
              TextField(
                key: const Key('assistant-provider-url'),
                controller: _baseUrl,
                autocorrect: false,
                decoration: const InputDecoration(
                  labelText: 'API base URL',
                  hintText: 'https://example.com/v1',
                ),
                onChanged: (_) => setState(() {}),
              ),
              const SizedBox(height: 16),
              TextField(
                key: const Key('assistant-provider-model'),
                controller: _modelId,
                autocorrect: false,
                decoration: const InputDecoration(
                  labelText: 'Model ID (optional)',
                  helperText: 'Leave empty to discover available models.',
                ),
              ),
              if (_provider!.description.isNotEmpty) ...[
                const SizedBox(height: 12),
                Text(_provider!.description),
              ],
            ],
            const SizedBox(height: 12),
            const Text(
              'Messages and requested file contents go to this API. Free access and rate limits depend on the provider; LangGraph does not require a subscription.',
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
      if (_provider?.connected == true ||
          (_providerId != null && widget.settingsFor(_providerId!).isNotEmpty))
        TextButton.icon(
          key: const Key('assistant-remove-provider'),
          icon: const Icon(Icons.delete_outline),
          label: const Text('Remove API'),
          onPressed: () => Navigator.pop(
            context,
            _ProviderConnection(_providerId!, '', '', '', remove: true),
          ),
        ),
      FilledButton(
        onPressed:
            _providerId == null ||
                (_provider!.requiresApiKey && _key.text.trim().isEmpty) ||
                (_providerId == 'custom' && _baseUrl.text.trim().isEmpty)
            ? null
            : () => Navigator.pop(
                context,
                _ProviderConnection(
                  _providerId!,
                  _key.text.trim(),
                  _baseUrl.text.trim(),
                  _modelId.text.trim(),
                ),
              ),
        child: const Text('Connect'),
      ),
    ],
  );
}

/// A full-width, keyboard-accessible trigger shared by both selection lists.
class _SelectionField extends StatelessWidget {
  const _SelectionField({
    super.key,
    required this.label,
    this.value,
    this.onPressed,
  });
  final String label;
  final String? value;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) => OutlinedButton(
    onPressed: onPressed,
    style: OutlinedButton.styleFrom(padding: const EdgeInsets.all(12)),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: Theme.of(context).textTheme.labelSmall),
        const SizedBox(height: 4),
        Row(
          children: [
            Expanded(
              child: Text(
                value ?? 'Choose ${label.toLowerCase()}',
                overflow: TextOverflow.ellipsis,
              ),
            ),
            const Icon(Icons.arrow_drop_down),
          ],
        ),
      ],
    ),
  );
}

class _Choice {
  const _Choice({
    required this.id,
    required this.label,
    required this.detail,
    this.keywords = '',
  });
  final String id, label, detail, keywords;
  String get searchable => '$label $detail $id $keywords'.toLowerCase();
}

class _ChoiceDialog extends StatefulWidget {
  const _ChoiceDialog({
    required this.title,
    required this.searchHint,
    required this.emptyMessage,
    required this.choices,
    this.selectedId,
  });
  final String title, searchHint, emptyMessage;
  final List<_Choice> choices;
  final String? selectedId;

  @override
  State<_ChoiceDialog> createState() => _ChoiceDialogState();
}

class _ChoiceDialogState extends State<_ChoiceDialog> {
  final _search = TextEditingController();
  late final _choices = List<_Choice>.of(widget.choices)
    ..sort((a, b) {
      final byName = a.label.toLowerCase().compareTo(b.label.toLowerCase());
      if (byName != 0) return byName;
      final byDetail = a.detail.toLowerCase().compareTo(b.detail.toLowerCase());
      return byDetail != 0 ? byDetail : a.id.compareTo(b.id);
    });

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final terms = _search.text.trim().toLowerCase().split(RegExp(r'\s+'));
    final matches = _choices
        .where((choice) => terms.every(choice.searchable.contains))
        .toList();
    return AlertDialog(
      title: Text(widget.title),
      content: SizedBox(
        width: 480,
        height: MediaQuery.sizeOf(context).height * .5,
        child: Column(
          children: [
            TextField(
              key: const Key('assistant-selection-search'),
              controller: _search,
              autofocus: true,
              onChanged: (_) => setState(() {}),
              decoration: InputDecoration(
                hintText: widget.searchHint,
                prefixIcon: const Icon(Icons.search),
                suffixIcon: _search.text.isEmpty
                    ? null
                    : IconButton(
                        tooltip: 'Clear search',
                        onPressed: () => setState(_search.clear),
                        icon: const Icon(Icons.clear),
                      ),
              ),
            ),
            const SizedBox(height: 12),
            Expanded(
              child: matches.isEmpty
                  ? Center(
                      child: Text(
                        _choices.isEmpty
                            ? widget.emptyMessage
                            : 'No matches found.',
                      ),
                    )
                  : ListView.builder(
                      key: ValueKey('assistant-choices-${_search.text}'),
                      itemCount: matches.length,
                      itemBuilder: (context, index) {
                        final choice = matches[index];
                        return ListTile(
                          key: ValueKey('assistant-choice-${choice.id}'),
                          title: Text(choice.label),
                          subtitle: Text(choice.detail),
                          selected: choice.id == widget.selectedId,
                          trailing: choice.id == widget.selectedId
                              ? const Icon(Icons.check)
                              : null,
                          onTap: () => Navigator.pop(context, choice.id),
                        );
                      },
                    ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}
