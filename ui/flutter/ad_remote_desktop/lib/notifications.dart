import 'dart:async';
import 'dart:collection';

import 'package:flutter/material.dart';

import 'app_log.dart';

enum NoticeKind { status, information, warning, error }

class NoticePolicy {
  const NoticePolicy._();

  static const Duration standardDuration = Duration(seconds: 5);
  static const Duration fadeDuration = Duration(milliseconds: 180);
  static const int maxPendingNotifications = 256;

  static Duration durationFor(NoticeKind kind) => standardDuration;
}

class AetherNotice {
  const AetherNotice._();

  static void show(
    BuildContext context, {
    required String message,
    NoticeKind kind = NoticeKind.information,
    SnackBarAction? action,
  }) {
    // Every on-screen notice is mirrored into the application log: notices
    // disappear after five seconds, the log does not (log_feature.md C).
    switch (kind) {
      case NoticeKind.error:
        AppLog.instance.error(AppLogSource.uiDart, message);
      case NoticeKind.warning:
        AppLog.instance.warn(AppLogSource.uiDart, message);
      case NoticeKind.status || NoticeKind.information:
        AppLog.instance.info(AppLogSource.uiDart, message);
    }
    final host = _AetherNoticeScope.maybeOf(context);
    if (host != null) {
      host.enqueue(message: message, kind: kind, action: action);
      return;
    }

    // A safe fallback for embedders that have not installed AetherNoticeHost.
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        duration: NoticePolicy.durationFor(kind),
        // Flutter 3.47 makes every snackbar with an action persistent unless
        // this is explicitly false. That made Undo notifications immortal.
        persist: false,
        showCloseIcon: true,
        content: Text(message),
        action: action,
      ),
    );
  }
}

class AetherNoticeHost extends StatefulWidget {
  const AetherNoticeHost({super.key, required this.child});

  final Widget child;

  @override
  State<AetherNoticeHost> createState() => _AetherNoticeHostState();
}

class _NoticeEntry {
  _NoticeEntry({
    required this.id,
    required this.message,
    required this.kind,
    required this.action,
  });

  final int id;
  final String message;
  final NoticeKind kind;
  final SnackBarAction? action;
}

class _AetherNoticeHostState extends State<AetherNoticeHost> {
  final ListQueue<_NoticeEntry> _pending = ListQueue<_NoticeEntry>();
  _NoticeEntry? _current;
  Timer? _fadeTimer;
  Timer? _expiryTimer;
  bool _visible = false;
  int _nextId = 0;
  int _timerGeneration = 0;

  void enqueue({
    required String message,
    required NoticeKind kind,
    required SnackBarAction? action,
  }) {
    final entry = _NoticeEntry(
      id: _nextId++,
      message: message,
      kind: kind,
      action: action,
    );
    if (_current == null) {
      setState(() {
        _current = entry;
        _visible = false;
      });
      _startLifecycleAfterVisible(entry);
      return;
    }

    // Bound pending memory during a notification storm while preserving the
    // current notice and all ordinary (including 100-item) workloads.
    if (_pending.length >= NoticePolicy.maxPendingNotifications) {
      _pending.removeFirst();
    }
    _pending.addLast(entry);
  }

  void _startLifecycleAfterVisible(_NoticeEntry entry) {
    _cancelTimers();
    final generation = ++_timerGeneration;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted ||
          generation != _timerGeneration ||
          _current?.id != entry.id) {
        return;
      }
      setState(() => _visible = true);
      final lifetime = NoticePolicy.durationFor(entry.kind);
      _fadeTimer = Timer(lifetime - NoticePolicy.fadeDuration, () {
        if (mounted &&
            generation == _timerGeneration &&
            _current?.id == entry.id) {
          setState(() => _visible = false);
        }
      });
      _expiryTimer = Timer(lifetime, () {
        if (mounted &&
            generation == _timerGeneration &&
            _current?.id == entry.id) {
          _dismiss(entry);
        }
      });
    });
  }

  void _dismiss(_NoticeEntry entry) {
    if (_current?.id != entry.id) return;
    _cancelTimers();
    _timerGeneration++;
    final next = _pending.isEmpty ? null : _pending.removeFirst();
    setState(() {
      _current = next;
      _visible = false;
    });
    if (next != null) _startLifecycleAfterVisible(next);
  }

  void _cancelTimers() {
    _fadeTimer?.cancel();
    _fadeTimer = null;
    _expiryTimer?.cancel();
    _expiryTimer = null;
  }

  void _runAction(_NoticeEntry entry) {
    try {
      entry.action?.onPressed();
    } finally {
      _dismiss(entry);
    }
  }

  @override
  void dispose() {
    _timerGeneration++;
    _cancelTimers();
    _pending.clear();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final entry = _current;
    return _AetherNoticeScope(
      host: this,
      child: Stack(
        fit: StackFit.expand,
        children: [
          widget.child,
          if (entry != null)
            Align(
              alignment: Alignment.bottomCenter,
              child: AnimatedOpacity(
                key: ValueKey('aether-notice-${entry.id}'),
                opacity: _visible ? 1 : 0,
                duration: NoticePolicy.fadeDuration,
                curve: Curves.easeOutCubic,
                child: _NoticeView(
                  entry: entry,
                  onAction: () => _runAction(entry),
                  onDismiss: () => _dismiss(entry),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _AetherNoticeScope extends InheritedWidget {
  const _AetherNoticeScope({required this.host, required super.child});

  final _AetherNoticeHostState host;

  static _AetherNoticeHostState? maybeOf(BuildContext context) {
    return context.getInheritedWidgetOfExactType<_AetherNoticeScope>()?.host;
  }

  @override
  bool updateShouldNotify(_AetherNoticeScope oldWidget) => false;
}

class _NoticeView extends StatelessWidget {
  const _NoticeView({
    required this.entry,
    required this.onAction,
    required this.onDismiss,
  });

  final _NoticeEntry entry;
  final VoidCallback onAction;
  final VoidCallback onDismiss;

  IconData get _icon => switch (entry.kind) {
    NoticeKind.status => Icons.check_circle_outline_rounded,
    NoticeKind.information => Icons.info_outline_rounded,
    NoticeKind.warning => Icons.warning_amber_rounded,
    NoticeKind.error => Icons.error_outline_rounded,
  };

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return SafeArea(
      top: false,
      child: Material(
        color: colors.inverseSurface,
        elevation: 8,
        child: Semantics(
          container: true,
          liveRegion: true,
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 48),
            child: Padding(
              padding: const EdgeInsets.only(left: 20, right: 8),
              child: Row(
                children: [
                  Icon(_icon, size: 19, color: colors.onInverseSurface),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(
                      entry.message,
                      style: TextStyle(color: colors.onInverseSurface),
                    ),
                  ),
                  if (entry.action case final action?)
                    TextButton(onPressed: onAction, child: Text(action.label)),
                  IconButton(
                    tooltip: 'Dismiss notification',
                    onPressed: onDismiss,
                    color: colors.onInverseSurface,
                    icon: const Icon(Icons.close_rounded),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// An inline notice with the same lifetime rules as snackbar notifications.
/// The timer is owned by this widget and is always cancelled when it is
/// replaced or disposed, so closed tabs cannot retain callbacks.
class AutoDismissBanner extends StatefulWidget {
  const AutoDismissBanner({
    super.key,
    required this.message,
    required this.onDismiss,
    this.kind = NoticeKind.error,
    this.leading,
    this.actions = const [],
  });

  final String message;
  final VoidCallback onDismiss;
  final NoticeKind kind;
  final Widget? leading;
  final List<Widget> actions;

  @override
  State<AutoDismissBanner> createState() => _AutoDismissBannerState();
}

class _AutoDismissBannerState extends State<AutoDismissBanner> {
  Timer? _fadeTimer;
  Timer? _expiryTimer;
  bool _visible = false;
  int _scheduleGeneration = 0;

  @override
  void initState() {
    super.initState();
    _scheduleDismissal();
  }

  @override
  void didUpdateWidget(covariant AutoDismissBanner oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.message != widget.message || oldWidget.kind != widget.kind) {
      _scheduleDismissal();
    }
  }

  void _scheduleDismissal() {
    _cancelTimers();
    if (mounted) setState(() => _visible = false);
    final generation = ++_scheduleGeneration;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || generation != _scheduleGeneration) return;
      setState(() => _visible = true);
      final lifetime = NoticePolicy.durationFor(widget.kind);
      _fadeTimer = Timer(lifetime - NoticePolicy.fadeDuration, () {
        if (mounted && generation == _scheduleGeneration) {
          setState(() => _visible = false);
        }
      });
      _expiryTimer = Timer(lifetime, () {
        if (mounted && generation == _scheduleGeneration) widget.onDismiss();
      });
    });
  }

  void _cancelTimers() {
    _fadeTimer?.cancel();
    _fadeTimer = null;
    _expiryTimer?.cancel();
    _expiryTimer = null;
  }

  @override
  void dispose() {
    _scheduleGeneration++;
    _cancelTimers();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedOpacity(
      opacity: _visible ? 1 : 0,
      duration: NoticePolicy.fadeDuration,
      curve: Curves.easeOutCubic,
      child: MaterialBanner(
        content: Text(widget.message),
        leading: widget.leading,
        actions: [
          ...widget.actions,
          IconButton(
            tooltip: 'Dismiss',
            onPressed: widget.onDismiss,
            icon: const Icon(Icons.close_rounded),
          ),
        ],
      ),
    );
  }
}
