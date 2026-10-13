import 'package:ad_remote_desktop/notifications.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'status, information, and warning notices dismiss after five seconds',
    () {
      for (final kind in const [
        NoticeKind.status,
        NoticeKind.information,
        NoticeKind.warning,
      ]) {
        expect(NoticePolicy.durationFor(kind), const Duration(seconds: 5));
      }
    },
  );

  test('error notices also dismiss after exactly five seconds', () {
    expect(
      NoticePolicy.durationFor(NoticeKind.error),
      const Duration(seconds: 5),
    );
  });

  testWidgets('actionable notifications dismiss after exactly five seconds', (
    tester,
  ) async {
    await tester.pumpWidget(const _NoticeHarness());

    await tester.tap(find.text('Show actionable notice'));
    await tester.pump();
    expect(find.text('Profile deleted'), findsOneWidget);

    await tester.pump(const Duration(milliseconds: 4999));
    expect(find.text('Profile deleted'), findsOneWidget);

    await tester.pump(const Duration(milliseconds: 1));
    expect(find.text('Profile deleted'), findsNothing);
  });

  testWidgets('notification begins fading before its five-second expiry', (
    tester,
  ) async {
    await tester.pumpWidget(const _NoticeHarness());

    await tester.tap(find.text('Show actionable notice'));
    await tester.pump();
    await tester.pump(
      NoticePolicy.standardDuration - NoticePolicy.fadeDuration,
    );

    final opacity = tester.widget<AnimatedOpacity>(
      find.ancestor(
        of: find.text('Profile deleted'),
        matching: find.byType(AnimatedOpacity),
      ),
    );
    expect(opacity.opacity, 0);

    await tester.pump(NoticePolicy.fadeDuration);
    expect(find.text('Profile deleted'), findsNothing);
  });

  testWidgets('notifications use a stable first-in first-out queue', (
    tester,
  ) async {
    await tester.pumpWidget(const _NoticeHarness());

    await tester.tap(find.text('Queue notices'));
    await tester.pump();
    expect(find.text('Notice 1'), findsOneWidget);
    expect(find.text('Notice 2'), findsNothing);

    await tester.pump(const Duration(seconds: 5));
    expect(find.text('Notice 1'), findsNothing);
    expect(find.text('Notice 2'), findsOneWidget);

    await tester.pump(const Duration(seconds: 5));
    expect(find.text('Notice 2'), findsNothing);
    expect(find.text('Notice 3'), findsOneWidget);

    await tester.pump(const Duration(seconds: 5));
    expect(find.text('Notice 3'), findsNothing);
    expect(find.byTooltip('Dismiss notification'), findsNothing);
  });

  testWidgets('manual close advances the queue and cancels the old timer', (
    tester,
  ) async {
    await tester.pumpWidget(const _NoticeHarness());
    await tester.tap(find.text('Queue notices'));
    await tester.pump();

    await tester.tap(find.byTooltip('Dismiss notification'));
    await tester.pump();
    expect(find.text('Notice 1'), findsNothing);
    expect(find.text('Notice 2'), findsOneWidget);

    await tester.pump(const Duration(milliseconds: 4999));
    expect(find.text('Notice 2'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 1));
    expect(find.text('Notice 2'), findsNothing);
    expect(find.text('Notice 3'), findsOneWidget);
  });

  testWidgets('a 100-notification burst drains without stale entries', (
    tester,
  ) async {
    await tester.pumpWidget(const _NoticeHarness());
    await tester.tap(find.text('Queue 100 notices'));
    await tester.pump();

    for (var index = 1; index <= 100; index++) {
      expect(find.text('Stress $index'), findsOneWidget);
      await tester.pump(const Duration(seconds: 5));
    }
    expect(find.byTooltip('Dismiss notification'), findsNothing);
  });

  testWidgets('inline notices auto-dismiss and cancel timers on disposal', (
    tester,
  ) async {
    var dismissals = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: AutoDismissBanner(
          message: 'Connected',
          kind: NoticeKind.status,
          onDismiss: () => dismissals++,
        ),
      ),
    );

    await tester.pump(const Duration(seconds: 5));
    expect(dismissals, 1);

    dismissals = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: AutoDismissBanner(
          message: 'Transient warning',
          kind: NoticeKind.warning,
          onDismiss: () => dismissals++,
        ),
      ),
    );
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump(const Duration(seconds: 6));
    expect(dismissals, 0);
  });
}

class _NoticeHarness extends StatelessWidget {
  const _NoticeHarness();

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      home: AetherNoticeHost(
        child: Scaffold(
          body: Builder(
            builder: (context) => Column(
              children: [
                TextButton(
                  onPressed: () => AetherNotice.show(
                    context,
                    message: 'Profile deleted',
                    action: SnackBarAction(label: 'Undo', onPressed: () {}),
                  ),
                  child: const Text('Show actionable notice'),
                ),
                TextButton(
                  onPressed: () {
                    for (var index = 1; index <= 3; index++) {
                      AetherNotice.show(context, message: 'Notice $index');
                    }
                  },
                  child: const Text('Queue notices'),
                ),
                TextButton(
                  onPressed: () {
                    for (var index = 1; index <= 100; index++) {
                      AetherNotice.show(context, message: 'Stress $index');
                    }
                  },
                  child: const Text('Queue 100 notices'),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
