import 'dart:async';
import 'dart:io';

import 'package:ad_stream_surface/ad_stream_surface.dart';

import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'remote_desktop.dart';

/// Convert USB HID keyboard usages to Linux evdev key codes.
int? remoteScanCode(PhysicalKeyboardKey key) {
  final usage = key.usbHidUsage;
  if (usage >> 16 != 7) return null;
  const codes = <int, int>{
    4: 30,
    5: 48,
    6: 46,
    7: 32,
    8: 18,
    9: 33,
    10: 34,
    11: 35,
    12: 23,
    13: 36,
    14: 37,
    15: 38,
    16: 50,
    17: 49,
    18: 24,
    19: 25,
    20: 16,
    21: 19,
    22: 31,
    23: 20,
    24: 22,
    25: 47,
    26: 17,
    27: 45,
    28: 21,
    29: 44,
    30: 2,
    31: 3,
    32: 4,
    33: 5,
    34: 6,
    35: 7,
    36: 8,
    37: 9,
    38: 10,
    39: 11,
    40: 28,
    41: 1,
    42: 14,
    43: 15,
    44: 57,
    45: 12,
    46: 13,
    47: 26,
    48: 27,
    49: 43,
    50: 86,
    51: 39,
    52: 40,
    53: 41,
    54: 51,
    55: 52,
    56: 53,
    57: 58,
    58: 59,
    59: 60,
    60: 61,
    61: 62,
    62: 63,
    63: 64,
    64: 65,
    65: 66,
    66: 67,
    67: 68,
    68: 87,
    69: 88,
    70: 99,
    71: 70,
    72: 119,
    73: 110,
    74: 102,
    75: 104,
    76: 111,
    77: 107,
    78: 109,
    79: 106,
    80: 105,
    81: 108,
    82: 103,
    83: 69,
    84: 98,
    85: 55,
    86: 74,
    87: 78,
    88: 96,
    89: 79,
    90: 80,
    91: 81,
    92: 75,
    93: 76,
    94: 77,
    95: 71,
    96: 72,
    97: 73,
    98: 82,
    99: 83,
    100: 86,
    101: 127,
    224: 29,
    225: 42,
    226: 56,
    227: 125,
    228: 97,
    229: 54,
    230: 100,
    231: 126,
  };
  return codes[usage & 0xffff];
}

/// Carry authoritative lock state even when locks were toggled outside the
/// remote view. Physical modifier keys continue to be forwarded individually.
int remoteLockModifiers(Set<KeyboardLockMode> locks) =>
    0x8000 |
    (locks.contains(KeyboardLockMode.numLock) ? 0x10 : 0) |
    (locks.contains(KeyboardLockMode.capsLock) ? 0x20 : 0);

Offset remotePoint(Offset local, Size view, Size remote) => Offset(
  (local.dx * remote.width / view.width).clamp(0, remote.width - 1),
  (local.dy * remote.height / view.height).clamp(0, remote.height - 1),
);

// Match physical monitor pixels while respecting the encoder's even-size
// and framebuffer limits. Scaling both axes preserves the viewport's shape.
Size remoteTabSize(Size available, double pixelRatio) {
  final width = available.width * pixelRatio;
  final height = available.height * pixelRatio;
  final scale = math.min(1.0, math.min(7680 / width, 4320 / height));
  return Size(
    ((width * scale).floor().clamp(320, 7680) ~/ 2) * 2.0,
    ((height * scale).floor().clamp(200, 4320) ~/ 2) * 2.0,
  );
}

class RemoteDesktopView extends StatefulWidget {
  const RemoteDesktopView({super.key, required this.session});
  final RemoteDesktopSession session;
  @override
  State<RemoteDesktopView> createState() => _RemoteDesktopViewState();
}

class _RemoteDesktopViewState extends State<RemoteDesktopView>
    with WidgetsBindingObserver {
  final FocusNode _focus = FocusNode(debugLabel: 'remote desktop');
  final Set<int> _keys = {};
  int _buttons = 0;
  Offset _position = Offset.zero;
  Timer? _clipboardTimer;
  Timer? _layoutTimer;
  Timer? _resizeTimer;
  Timer? _moveTimer;
  bool _movePending = false;
  Size? _tabSize;
  String? _keyboardLayout;
  bool _layoutBusy = false;
  bool _clipboardBusy = false;
  bool _clipboardEnabled = true;
  String? _lastClipboard;
  String? _error;
  bool _disposing = false;
  bool _active = true;
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    widget.session.closed.then((_) {
      if (!mounted) return;
      _disposing = true;
      _clipboardTimer?.cancel();
      _layoutTimer?.cancel();
      _resizeTimer?.cancel();
      _moveTimer?.cancel();
      _moveTimer = null;
      _movePending = false;
      _releaseInput();
    });
    unawaited(_syncKeyboardLayout());
    _layoutTimer = Timer.periodic(const Duration(milliseconds: 500), (_) {
      if (_active && _focus.hasFocus) unawaited(_syncKeyboardLayout());
    });
    _clipboardTimer = Timer.periodic(
      const Duration(milliseconds: 750),
      (_) => _syncClipboard(),
    );
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _active = state == AppLifecycleState.resumed;
    if (!_active) _releaseInput();
  }

  bool _send(Map<String, Object?> input) {
    try {
      widget.session.sendInput(input);
      return true;
    } on Object catch (error) {
      if (mounted && !_disposing && _error != error.toString()) {
        setState(() => _error = error.toString());
      }
      return false;
    }
  }

  bool _key(int code, bool pressed, {bool repeat = false}) => _send({
    'Keyboard': {
      'scan_code': code,
      'pressed': pressed,
      'modifiers': remoteLockModifiers(
        HardwareKeyboard.instance.lockModesEnabled,
      ),
      'repeat': repeat,
    },
  });
  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    // Ctrl+Alt+Escape releases keyboard capture without sending Escape remotely.
    if (event.logicalKey == LogicalKeyboardKey.escape &&
        HardwareKeyboard.instance.isControlPressed &&
        HardwareKeyboard.instance.isAltPressed) {
      _focus.unfocus();
      return KeyEventResult.handled;
    }
    final code = remoteScanCode(event.physicalKey);
    if (code == null) return KeyEventResult.ignored;
    if (event is KeyUpEvent) {
      if (_keys.contains(code) && _key(code, false)) _keys.remove(code);
    } else if (event is KeyDownEvent) {
      if (!_keys.contains(code) && _key(code, true)) _keys.add(code);
    }
    // Wayland clients handle repeat from the compositor's key-repeat metadata.
    return KeyEventResult.handled;
  }

  void _releaseInput() {
    _moveTimer?.cancel();
    _moveTimer = null;
    _movePending = false;
    for (final code in _keys.toList()) {
      if (_key(code, false)) _keys.remove(code);
    }
    _setButtons(0);
  }

  void _setButtons(int next) {
    for (final entry in {
      kPrimaryMouseButton: 1,
      kSecondaryMouseButton: 2,
      kMiddleMouseButton: 3,
      kBackMouseButton: 4,
      kForwardMouseButton: 5,
    }.entries) {
      if ((_buttons & entry.key) != (next & entry.key)) {
        if (_send({
          'PointerButton': {
            'button': entry.value,
            'pressed': next & entry.key != 0,
            'x': _position.dx.toInt(),
            'y': _position.dy.toInt(),
          },
        })) {
          _buttons = (_buttons & ~entry.key) | (next & entry.key);
        }
      }
    }
  }

  void _move(PointerEvent event, Size view, Size remote) {
    _position = remotePoint(event.localPosition, view, remote);
    // Coalesce pointer motion to ~60 Hz (increase_speed.md B8): a fast mouse
    // otherwise floods the same channel that carries frames and input acks.
    // The first move goes out immediately; the latest position is always
    // delivered within one interval.
    if (_moveTimer != null) {
      _movePending = true;
      return;
    }
    _sendMove();
    _moveTimer = Timer(const Duration(milliseconds: 16), () {
      _moveTimer = null;
      if (_movePending) {
        _movePending = false;
        _sendMove();
      }
    });
  }

  void _sendMove() {
    _send({
      'PointerMove': {
        'x': _position.dx.toInt(),
        'y': _position.dy.toInt(),
        'monitor': 0,
      },
    });
  }

  Future<void> _syncKeyboardLayout({bool force = false}) async {
    if (!Platform.isWindows || _layoutBusy || _disposing) return;
    _layoutBusy = true;
    try {
      final mapping = await AdStreamSurface.keyboardLayout();
      if (!mounted || _disposing || mapping == null) return;
      final layout = mapping['layout']!;
      final variant = mapping['variant'] ?? '';
      final signature = '$layout/$variant';
      if (force || _keyboardLayout != signature) {
        _releaseInput();
        widget.session.setKeyboardLayout(layout, variant);
        _keyboardLayout = signature;
      }
    } on MissingPluginException {
      // Widget tests have no Windows platform plugin.
    } on Object catch (error) {
      if (mounted && !_disposing) setState(() => _error = error.toString());
    } finally {
      _layoutBusy = false;
    }
  }

  void _scheduleTabSize(Size available, double pixelRatio) {
    if (_disposing || !available.isFinite || available.isEmpty) return;
    final size = remoteTabSize(available, pixelRatio);
    if (_tabSize == size) return;
    _tabSize = size;
    _resizeTimer?.cancel();
    _resizeTimer = Timer(const Duration(milliseconds: 150), () {
      if (mounted && !_disposing) unawaited(_applyTabSize());
    });
  }

  Future<void> _applyTabSize({bool force = false}) async {
    _resizeTimer?.cancel();
    final size = _tabSize;
    if (size == null || _disposing) return;
    try {
      if (Platform.isWindows) {
        try {
          await AdStreamSurface.setTabSize(
            widget.session.textureId,
            size.width.toInt(),
            size.height.toInt(),
            force: force,
          );
          return;
        } on MissingPluginException {
          // Widget tests and platforms without native presentation use the session.
        }
      }
      widget.session.resize(size.width.toInt(), size.height.toInt());
    } on Object catch (error) {
      if (mounted && !_disposing) setState(() => _error = error.toString());
    }
  }

  Future<void> _present(bool allMonitors) async {
    _releaseInput();
    _focus.unfocus();
    try {
      await _applyTabSize();
      await _syncKeyboardLayout(force: true);
      await AdStreamSurface.present(
        widget.session.textureId,
        allMonitors: allMonitors,
      );
    } on Object catch (error) {
      if (mounted) setState(() => _error = error.toString());
    }
  }

  bool _endingSession = false;
  Future<void> _endSession() async {
    if (_endingSession) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('End remote desktop session?'),
        content: const Text(
          'This closes the host applications and stops work running in this desktop. Close the connection tab instead to leave your work running.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('End session'),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;
    setState(() => _endingSession = true);
    _releaseInput();
    try {
      await widget.session.dispose(terminate: true);
    } on Object catch (error) {
      if (mounted) setState(() => _error = error.toString());
    } finally {
      if (mounted) setState(() => _endingSession = false);
    }
  }

  Future<void> _syncClipboard() async {
    if (!_clipboardEnabled || _clipboardBusy || _disposing) return;
    _clipboardBusy = true;
    try {
      var focused = _active && _focus.hasFocus;
      if (!focused && Platform.isWindows) {
        try {
          focused = await AdStreamSurface.isPresented(widget.session.textureId);
        } on MissingPluginException {
          focused = false;
        }
      }
      if (!focused || !mounted || _disposing) return;
      final remote = widget.session.takeClipboard();
      if (remote != null) {
        _lastClipboard = remote;
        await Clipboard.setData(ClipboardData(text: remote));
      } else {
        final local = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
        if (mounted &&
            !_disposing &&
            focused &&
            local != null &&
            local != _lastClipboard &&
            local.length <= 1000000) {
          widget.session.publishClipboard(local);
          _lastClipboard = local;
        }
      }
    } on Object catch (error) {
      if (mounted) setState(() => _error = error.toString());
    } finally {
      _clipboardBusy = false;
    }
  }

  @override
  void dispose() {
    _disposing = true;
    WidgetsBinding.instance.removeObserver(this);
    _clipboardTimer?.cancel();
    _layoutTimer?.cancel();
    _resizeTimer?.cancel();
    _moveTimer?.cancel();
    _moveTimer = null;
    _movePending = false;
    _releaseInput();
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(
    BuildContext context,
  ) => ValueListenableBuilder<RemoteDesktopStatus>(
    valueListenable: widget.session.status,
    builder: (context, state, _) => LayoutBuilder(
      builder: (context, constraints) {
        final remote = Size(state.width.toDouble(), state.height.toDouble());
        final available = Size(
          constraints.maxWidth,
          math.max(1, constraints.maxHeight - 42),
        );
        _scheduleTabSize(available, MediaQuery.devicePixelRatioOf(context));
        final scale = math.min(
          available.width / remote.width,
          available.height / remote.height,
        );
        final view = Size(remote.width * scale, remote.height * scale);
        return Column(
          children: [
            SizedBox(
              height: 42,
              child: Row(
                children: [
                  const SizedBox(width: 12),
                  Text('${state.width} × ${state.height}'),
                  const SizedBox(width: 12),
                  const Expanded(
                    child: Text(
                      'Click the desktop to type · Ctrl+Alt+Esc releases the keyboard',
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                  IconButton(
                    tooltip: 'Full screen',
                    icon: const Icon(Icons.fullscreen),
                    onPressed: () => _present(false),
                  ),
                  IconButton(
                    tooltip: 'Use all monitors',
                    icon: const Icon(Icons.desktop_windows_outlined),
                    onPressed: () => _present(true),
                  ),
                  TextButton.icon(
                    onPressed: _endingSession ? null : _endSession,
                    icon: const Icon(Icons.stop_circle_outlined),
                    label: Text(
                      _endingSession ? 'Ending session…' : 'End remote session',
                    ),
                  ),
                  IconButton(
                    tooltip: _clipboardEnabled
                        ? 'Pause clipboard sharing'
                        : 'Share text clipboard',
                    onPressed: () =>
                        setState(() => _clipboardEnabled = !_clipboardEnabled),
                    icon: Icon(
                      _clipboardEnabled
                          ? Icons.content_paste
                          : Icons.content_paste_off,
                    ),
                  ),
                  IconButton(
                    tooltip: 'Resize remote desktop to this tab',
                    icon: const Icon(Icons.fit_screen),
                    onPressed: () => _applyTabSize(force: true),
                  ),
                ],
              ),
            ),
            Expanded(
              child: ColoredBox(
                color: Colors.black,
                child: Center(
                  child: SizedBox(
                    width: view.width,
                    height: view.height,
                    child: Focus(
                      focusNode: _focus,
                      onKeyEvent: _onKey,
                      onFocusChange: (focused) {
                        if (!focused)
                          _releaseInput();
                        else
                          unawaited(_syncKeyboardLayout(force: true));
                      },
                      child: MouseRegion(
                        cursor: SystemMouseCursors.basic,
                        child: Listener(
                          behavior: HitTestBehavior.opaque,
                          onPointerHover: (event) => _move(event, view, remote),
                          onPointerMove: (event) {
                            _move(event, view, remote);
                            _setButtons(event.buttons);
                          },
                          onPointerDown: (event) {
                            _focus.requestFocus();
                            _move(event, view, remote);
                            _setButtons(event.buttons);
                          },
                          onPointerUp: (event) {
                            _move(event, view, remote);
                            _setButtons(event.buttons);
                          },
                          onPointerCancel: (_) => _setButtons(0),
                          onPointerSignal: (event) {
                            if (event is PointerScrollEvent)
                              _send({
                                'PointerWheel': {
                                  'horizontal': event.scrollDelta.dx.round(),
                                  'vertical': event.scrollDelta.dy.round(),
                                },
                              });
                          },
                          child: Stack(
                            fit: StackFit.expand,
                            children: [
                              Texture(
                                textureId: widget.session.textureId,
                                filterQuality: FilterQuality.low,
                              ),
                              if (state.frames == 0)
                                const Center(
                                  child: Column(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      CircularProgressIndicator(),
                                      SizedBox(height: 12),
                                      Text(
                                        'Waiting for the first desktop frame',
                                      ),
                                    ],
                                  ),
                                ),
                              if (_error != null || state.error != null)
                                Align(
                                  alignment: Alignment.bottomCenter,
                                  child: Material(
                                    color: Colors.red.shade900,
                                    child: Padding(
                                      padding: const EdgeInsets.all(8),
                                      child: Text(state.error ?? _error!),
                                    ),
                                  ),
                                ),
                            ],
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ],
        );
      },
    ),
  );
}
