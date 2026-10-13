import 'package:ad_remote_desktop/session_tabs.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('opening sessions always appends and preserves existing tabs', () {
    final tabs = SessionTabCollection<String>()..openNew('server-1:first');

    tabs
      ..openNew('server-1:first')
      ..openNew('server-2:first');

    expect(tabs.items, ['server-1:first', 'server-1:first', 'server-2:first']);
  });

  test('keyboard navigation wraps in both directions', () {
    expect(wrappedTabIndex(currentIndex: 4, tabCount: 5, delta: 1), 0);
    expect(wrappedTabIndex(currentIndex: 0, tabCount: 5, delta: -1), 4);
    expect(wrappedTabIndex(currentIndex: 0, tabCount: 0, delta: 1), -1);
  });

  test('active connections exclude disconnected and failed tabs', () {
    final states = ['connecting', 'connected', 'disconnected', 'failed'];

    expect(
      activeConnectionTabs(
        states,
        (state) => state == 'connecting' || state == 'connected',
      ),
      ['connecting', 'connected'],
    );
  });

  testWidgets('overflow controls expose and reveal any of 100 tabs', (
    tester,
  ) async {
    final tabs = List<String>.generate(100, (index) => 'tab-$index');
    var selected = tabs.first;
    var itemBuilds = 0;
    late StateSetter update;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 680,
              child: StatefulBuilder(
                builder: (context, setState) {
                  update = setState;
                  return SessionTabStrip<String>(
                    tabs: tabs,
                    selectedId: selected,
                    idOf: (tab) => tab,
                    onSelected: (tab) => setState(() => selected = tab),
                    onReorder: (oldIndex, newIndex) {
                      setState(() {
                        final tab = tabs.removeAt(oldIndex);
                        tabs.insert(newIndex, tab);
                      });
                    },
                    itemBuilder: (context, tab, isSelected) {
                      itemBuilds++;
                      return ColoredBox(
                        color: isSelected ? Colors.deepPurple : Colors.black12,
                        child: Text(tab),
                      );
                    },
                  );
                },
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(
      find.byKey(const ValueKey('session-tabs-scroll-left')),
      findsOneWidget,
    );
    expect(
      find.byKey(const ValueKey('session-tabs-scroll-right')),
      findsOneWidget,
    );
    expect(itemBuilds, lessThan(100), reason: 'the tab list must remain lazy');

    await tester.tap(find.byKey(const ValueKey('session-tabs-scroll-right')));
    await tester.pumpAndSettle();
    final leftButton = tester.widget<IconButton>(
      find.descendant(
        of: find.byKey(const ValueKey('session-tabs-scroll-left')),
        matching: find.byType(IconButton),
      ),
    );
    expect(leftButton.onPressed, isNotNull);

    update(() => selected = tabs.last);
    await tester.pumpAndSettle();

    final viewport = tester.getRect(
      find.byKey(const ValueKey('session-tabs-viewport')),
    );
    final activeTab = tester.getRect(
      find.byKey(const ValueKey('session-tab-tab-99')),
    );
    expect(activeTab.left, greaterThanOrEqualTo(viewport.left - 0.5));
    expect(activeTab.right, lessThanOrEqualTo(viewport.right + 0.5));
  });
}
