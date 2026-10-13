import 'package:flutter/material.dart';

import 'terminal_output.dart';
import 'terminal_theme.dart';

class TerminalSettingsPage extends StatefulWidget {
  const TerminalSettingsPage({super.key, required this.manager});

  final TerminalThemeManager manager;

  @override
  State<TerminalSettingsPage> createState() => _TerminalSettingsPageState();
}

class _TerminalSettingsPageState extends State<TerminalSettingsPage> {
  late final Map<TerminalSemanticRole, TextEditingController> _colorControllers;
  late final TextEditingController _scrollbackController;

  TerminalAppearance get _appearance => widget.manager.appearance;

  @override
  void initState() {
    super.initState();
    _colorControllers = {
      for (final role in TerminalSemanticRole.values)
        role: TextEditingController(
          text: _toHex(_appearance.palette.semantic(role)),
        ),
    };
    _scrollbackController = TextEditingController(
      text: _appearance.scrollbackLines.toString(),
    );
  }

  @override
  void dispose() {
    for (final controller in _colorControllers.values) {
      controller.dispose();
    }
    _scrollbackController.dispose();
    super.dispose();
  }

  void _syncFields() {
    for (final role in TerminalSemanticRole.values) {
      _colorControllers[role]!.text = _toHex(
        _appearance.palette.semantic(role),
      );
    }
    _scrollbackController.text = _appearance.scrollbackLines.toString();
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return Dialog(
      clipBehavior: Clip.antiAlias,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 900, maxHeight: 760),
        child: Column(
          children: [
            Material(
              color: colors.surfaceContainerHigh,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(22, 15, 10, 15),
                child: Row(
                  children: [
                    Icon(Icons.palette_outlined, color: colors.primary),
                    const SizedBox(width: 10),
                    const Expanded(
                      child: Text(
                        'Terminal appearance',
                        style: TextStyle(
                          fontSize: 20,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ),
                    IconButton(
                      tooltip: 'Close settings',
                      onPressed: () => Navigator.pop(context),
                      icon: const Icon(Icons.close_rounded),
                    ),
                  ],
                ),
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.all(22),
                children: [
                  Wrap(
                    spacing: 18,
                    runSpacing: 14,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      SizedBox(
                        width: 270,
                        child: DropdownButtonFormField<TerminalThemePreset>(
                          initialValue: _appearance.preset,
                          decoration: const InputDecoration(
                            labelText: 'Theme preset',
                          ),
                          items: TerminalThemePreset.values
                              .map(
                                (preset) => DropdownMenuItem(
                                  value: preset,
                                  child: Text(preset.label),
                                ),
                              )
                              .toList(),
                          onChanged: (preset) {
                            if (preset == null) return;
                            setState(() {
                              widget.manager.applyPreset(preset);
                              _syncFields();
                            });
                          },
                        ),
                      ),
                      SizedBox(
                        width: 290,
                        child: SwitchListTile.adaptive(
                          contentPadding: EdgeInsets.zero,
                          title: const Text('Semantic log highlighting'),
                          subtitle: const Text(
                            'Preserves colors supplied by remote programs.',
                          ),
                          value: _appearance.autoClassify,
                          onChanged: (value) => setState(
                            () => widget.manager.updateAutoClassify(value),
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 18),
                  _TerminalPreview(appearance: _appearance),
                  const SizedBox(height: 24),
                  Text(
                    'Typography and history',
                    style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(height: 10),
                  Wrap(
                    spacing: 18,
                    runSpacing: 10,
                    children: [
                      SizedBox(
                        width: 310,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              'Font size · ${_appearance.fontSize.toStringAsFixed(0)} px',
                            ),
                            Slider(
                              value: _appearance.fontSize,
                              min: 10,
                              max: 28,
                              divisions: 18,
                              label: _appearance.fontSize.toStringAsFixed(0),
                              onChanged: (value) => setState(
                                () => widget.manager.updateFontSize(value),
                              ),
                            ),
                          ],
                        ),
                      ),
                      SizedBox(
                        width: 310,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              'Line height · ${_appearance.lineHeight.toStringAsFixed(1)}',
                            ),
                            Slider(
                              value: _appearance.lineHeight,
                              min: 1,
                              max: 2,
                              divisions: 10,
                              label: _appearance.lineHeight.toStringAsFixed(1),
                              onChanged: (value) => setState(
                                () => widget.manager.updateLineHeight(value),
                              ),
                            ),
                          ],
                        ),
                      ),
                      SizedBox(
                        width: 220,
                        child: TextFormField(
                          controller: _scrollbackController,
                          keyboardType: TextInputType.number,
                          decoration: const InputDecoration(
                            labelText: 'Scrollback lines',
                            helperText: '20,000–250,000',
                          ),
                          onFieldSubmitted: (value) {
                            final parsed = int.tryParse(value);
                            if (parsed == null) {
                              _scrollbackController.text = _appearance
                                  .scrollbackLines
                                  .toString();
                              return;
                            }
                            setState(() {
                              widget.manager.updateScrollbackLines(parsed);
                              _scrollbackController.text = _appearance
                                  .scrollbackLines
                                  .toString();
                            });
                          },
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 20),
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          'Semantic colors',
                          style: Theme.of(context).textTheme.titleMedium
                              ?.copyWith(fontWeight: FontWeight.w700),
                        ),
                      ),
                      TextButton.icon(
                        onPressed: () => setState(() {
                          widget.manager.applyPreset(_appearance.preset);
                          _syncFields();
                        }),
                        icon: const Icon(Icons.restart_alt_rounded),
                        label: const Text('Reset preset'),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  LayoutBuilder(
                    builder: (context, constraints) {
                      final columns = constraints.maxWidth >= 720 ? 2 : 1;
                      final width =
                          (constraints.maxWidth - (columns - 1) * 12) / columns;
                      return Wrap(
                        spacing: 12,
                        runSpacing: 10,
                        children: TerminalSemanticRole.values
                            .map(
                              (role) => SizedBox(
                                width: width,
                                child: _SemanticColorField(
                                  role: role,
                                  color: _appearance.palette.semantic(role),
                                  controller: _colorControllers[role]!,
                                  onSubmitted: (value) {
                                    final color = _parseHex(value);
                                    if (color == null) {
                                      _colorControllers[role]!.text = _toHex(
                                        _appearance.palette.semantic(role),
                                      );
                                      return;
                                    }
                                    setState(() {
                                      widget.manager.updateSemanticColor(
                                        role,
                                        color,
                                      );
                                      _colorControllers[role]!.text = _toHex(
                                        color,
                                      );
                                    });
                                  },
                                ),
                              ),
                            )
                            .toList(),
                      );
                    },
                  ),
                  const SizedBox(height: 12),
                  Text(
                    'Status labels and icons are retained so meaning never depends on color alone.',
                    style: TextStyle(
                      color: colors.onSurfaceVariant,
                      fontSize: 12,
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  static String _toHex(Color color) =>
      '#${(color.toARGB32() & 0xffffff).toRadixString(16).padLeft(6, '0').toUpperCase()}';

  static Color? _parseHex(String value) {
    final normalized = value.trim().replaceFirst('#', '');
    if (!RegExp(r'^[0-9a-fA-F]{6}$').hasMatch(normalized)) return null;
    return Color(0xff000000 | int.parse(normalized, radix: 16));
  }
}

class _TerminalPreview extends StatelessWidget {
  const _TerminalPreview({required this.appearance});

  final TerminalAppearance appearance;

  @override
  Widget build(BuildContext context) {
    const classifier = TerminalLogClassifier();
    const samples = [
      'Connected to build-server · SSH session opened',
      'Authentication successful · Ed25519 host key verified',
      'WARNING: package cache is nearing its size limit',
      'ERROR: service failed with exit code 1',
      'SFTP upload completed · 24.8 MB transferred',
      r'ashkan@build-server:~$ flutter test',
    ];
    return Semantics(
      label: 'Terminal theme preview',
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: appearance.palette.background,
          borderRadius: BorderRadius.circular(10),
          border: Border.all(color: appearance.palette.brightBlack),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: samples.map((line) {
            final role = classifier.classify(line);
            return Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Text(
                line,
                style: TextStyle(
                  color: role == null
                      ? appearance.palette.foreground
                      : appearance.palette.semantic(role),
                  fontSize: appearance.fontSize,
                  height: appearance.lineHeight,
                  fontFamily: 'Cascadia Mono',
                  fontFamilyFallback: const ['Consolas', 'monospace'],
                  fontWeight:
                      role == TerminalSemanticRole.error ||
                          role == TerminalSemanticRole.warning
                      ? FontWeight.bold
                      : FontWeight.normal,
                ),
              ),
            );
          }).toList(),
        ),
      ),
    );
  }
}

class _SemanticColorField extends StatelessWidget {
  const _SemanticColorField({
    required this.role,
    required this.color,
    required this.controller,
    required this.onSubmitted,
  });

  final TerminalSemanticRole role;
  final Color color;
  final TextEditingController controller;
  final ValueChanged<String> onSubmitted;

  @override
  Widget build(BuildContext context) {
    return TextFormField(
      controller: controller,
      onFieldSubmitted: onSubmitted,
      decoration: InputDecoration(
        labelText: role.label,
        prefixIcon: Padding(
          padding: const EdgeInsets.all(12),
          child: Semantics(
            label: '${role.label} color sample',
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: color,
                shape: BoxShape.circle,
                border: Border.all(
                  color: Theme.of(context).colorScheme.outline,
                ),
              ),
            ),
          ),
        ),
        suffixIcon: Icon(_iconFor(role)),
      ),
    );
  }

  static IconData _iconFor(TerminalSemanticRole role) => switch (role) {
    TerminalSemanticRole.error => Icons.error_outline_rounded,
    TerminalSemanticRole.warning => Icons.warning_amber_rounded,
    TerminalSemanticRole.success => Icons.check_circle_outline_rounded,
    TerminalSemanticRole.information => Icons.info_outline_rounded,
    TerminalSemanticRole.debug => Icons.bug_report_outlined,
    TerminalSemanticRole.connection => Icons.link_rounded,
    TerminalSemanticRole.disconnection => Icons.link_off_rounded,
    TerminalSemanticRole.fileTransfer => Icons.swap_vert_rounded,
    TerminalSemanticRole.authentication => Icons.verified_user_outlined,
    TerminalSemanticRole.prompt => Icons.terminal_rounded,
    TerminalSemanticRole.systemMessage => Icons.memory_rounded,
  };
}
