import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show ScrollCacheExtent;

/// Owns the invariant that opening a session always appends a new tab.
/// Reconnecting an existing tab is a separate operation and never calls this.
class SessionTabCollection<T> {
  final List<T> _items = [];

  List<T> get items => _items;

  void openNew(T tab) {
    _items.add(tab);
  }
}

/// Returns only tabs which currently own a live or opening connection.
/// Saved profiles deliberately do not participate in this collection.
List<T> activeConnectionTabs<T>(
  Iterable<T> tabs,
  bool Function(T tab) isActive,
) => tabs.where(isActive).toList(growable: false);

int wrappedTabIndex({
  required int currentIndex,
  required int tabCount,
  required int delta,
}) {
  if (tabCount <= 0) return -1;
  final normalizedCurrent = currentIndex.clamp(0, tabCount - 1);
  return (normalizedCurrent + delta) % tabCount;
}

/// A fixed-extent, lazy session tab viewport with browser-style overflow
/// controls. Keeping the extent fixed makes active-tab reveal O(1), including
/// for workspaces with hundreds of sessions.
class SessionTabStrip<T> extends StatefulWidget {
  const SessionTabStrip({
    super.key,
    required this.tabs,
    required this.selectedId,
    required this.idOf,
    required this.itemBuilder,
    required this.onSelected,
    required this.onReorder,
    this.height = 43,
    this.tabExtent = 220,
  });

  final List<T> tabs;
  final Object? selectedId;
  final Object Function(T tab) idOf;
  final Widget Function(BuildContext context, T tab, bool selected) itemBuilder;
  final ValueChanged<T> onSelected;
  final void Function(int oldIndex, int newIndex) onReorder;
  final double height;
  final double tabExtent;

  @override
  State<SessionTabStrip<T>> createState() => _SessionTabStripState<T>();
}

class _SessionTabStripState<T> extends State<SessionTabStrip<T>> {
  static const _edgePadding = 4.0;
  static const _scrollDuration = Duration(milliseconds: 180);

  final ScrollController _controller = ScrollController();
  bool _overflow = false;
  bool _canScrollLeft = false;
  bool _canScrollRight = false;
  double? _lastWidth;
  int _refreshGeneration = 0;

  @override
  void initState() {
    super.initState();
    _controller.addListener(_updateNavigationState);
    _scheduleRefresh(revealSelected: true);
  }

  @override
  void didUpdateWidget(covariant SessionTabStrip<T> oldWidget) {
    super.didUpdateWidget(oldWidget);
    final selectionChanged = oldWidget.selectedId != widget.selectedId;
    final layoutChanged =
        oldWidget.tabs.length != widget.tabs.length ||
        oldWidget.tabExtent != widget.tabExtent;
    if (selectionChanged || layoutChanged) {
      _scheduleRefresh(revealSelected: selectionChanged || layoutChanged);
    }
  }

  void _scheduleRefresh({required bool revealSelected}) {
    final generation = ++_refreshGeneration;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || generation != _refreshGeneration) return;
      _updateNavigationState();
      if (revealSelected) unawaited(_revealSelected());
    });
  }

  void _updateNavigationState() {
    if (!mounted || !_controller.hasClients) return;
    final position = _controller.position;
    final overflow = position.maxScrollExtent > 0.5;
    final canScrollLeft = position.pixels > position.minScrollExtent + 0.5;
    final canScrollRight = position.pixels < position.maxScrollExtent - 0.5;
    if (overflow != _overflow ||
        canScrollLeft != _canScrollLeft ||
        canScrollRight != _canScrollRight) {
      setState(() {
        _overflow = overflow;
        _canScrollLeft = canScrollLeft;
        _canScrollRight = canScrollRight;
      });
    }
  }

  Future<void> _revealSelected() async {
    if (!_controller.hasClients || widget.selectedId == null) return;
    final index = widget.tabs.indexWhere(
      (tab) => widget.idOf(tab) == widget.selectedId,
    );
    if (index < 0) return;

    final position = _controller.position;
    final itemStart = _edgePadding + (index * widget.tabExtent);
    final itemEnd = itemStart + widget.tabExtent;
    final viewportStart = position.pixels;
    final viewportEnd = viewportStart + position.viewportDimension;
    var target = viewportStart;
    if (itemStart < viewportStart) {
      target = itemStart - _edgePadding;
    } else if (itemEnd > viewportEnd) {
      target = itemEnd - position.viewportDimension + _edgePadding;
    }
    target = target.clamp(position.minScrollExtent, position.maxScrollExtent);
    if ((target - position.pixels).abs() <= 0.5) return;
    await _controller.animateTo(
      target,
      duration: _scrollDuration,
      curve: Curves.easeOutCubic,
    );
  }

  Future<void> _scrollByPage(int direction) async {
    if (!_controller.hasClients) return;
    final position = _controller.position;
    final target =
        (position.pixels + (position.viewportDimension * 0.8 * direction))
            .clamp(position.minScrollExtent, position.maxScrollExtent);
    await _controller.animateTo(
      target,
      duration: _scrollDuration,
      curve: Curves.easeOutCubic,
    );
  }

  @override
  void dispose() {
    _refreshGeneration++;
    _controller
      ..removeListener(_updateNavigationState)
      ..dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        if (_lastWidth != constraints.maxWidth) {
          _lastWidth = constraints.maxWidth;
          _scheduleRefresh(revealSelected: true);
        }
        return SizedBox(
          height: widget.height,
          child: Row(
            children: [
              if (_overflow)
                _TabNavigationButton(
                  key: const ValueKey('session-tabs-scroll-left'),
                  icon: Icons.chevron_left_rounded,
                  tooltip: 'Previous session tabs',
                  enabled: _canScrollLeft,
                  onPressed: () => unawaited(_scrollByPage(-1)),
                ),
              Expanded(
                child: ReorderableListView.builder(
                  key: const ValueKey('session-tabs-viewport'),
                  scrollController: _controller,
                  scrollDirection: Axis.horizontal,
                  buildDefaultDragHandles: false,
                  padding: const EdgeInsets.only(left: _edgePadding),
                  itemExtent: widget.tabExtent,
                  scrollCacheExtent: ScrollCacheExtent.pixels(
                    widget.tabExtent * 3,
                  ),
                  itemCount: widget.tabs.length,
                  onReorderItem: widget.onReorder,
                  itemBuilder: (context, index) {
                    final tab = widget.tabs[index];
                    final id = widget.idOf(tab);
                    final selected = id == widget.selectedId;
                    return ReorderableDragStartListener(
                      key: ValueKey('reorder-$id'),
                      index: index,
                      child: InkWell(
                        key: ValueKey('session-tab-$id'),
                        onTap: () => widget.onSelected(tab),
                        child: widget.itemBuilder(context, tab, selected),
                      ),
                    );
                  },
                ),
              ),
              if (_overflow)
                _TabNavigationButton(
                  key: const ValueKey('session-tabs-scroll-right'),
                  icon: Icons.chevron_right_rounded,
                  tooltip: 'Next session tabs',
                  enabled: _canScrollRight,
                  onPressed: () => unawaited(_scrollByPage(1)),
                ),
            ],
          ),
        );
      },
    );
  }
}

class _TabNavigationButton extends StatelessWidget {
  const _TabNavigationButton({
    super.key,
    required this.icon,
    required this.tooltip,
    required this.enabled,
    required this.onPressed,
  });

  final IconData icon;
  final String tooltip;
  final bool enabled;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 36,
      height: double.infinity,
      child: IconButton(
        tooltip: tooltip,
        onPressed: enabled ? onPressed : null,
        visualDensity: VisualDensity.compact,
        icon: Icon(icon, size: 19),
      ),
    );
  }
}
