import 'package:ad_remote_desktop/remote_directory.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('tracks complete and fragmented OSC 7 reports', () {
    final tracker = RemoteDirectoryTracker();

    final first = tracker.process('prompt\u001b]7;file://fic/home/AR/Doc');
    expect(first.paths, isEmpty);
    expect(first.visibleOutput, 'prompt');
    final second = tracker.process('uments\u0007next');
    expect(second.paths, ['/home/AR/Documents']);
    expect(second.visibleOutput, 'next');
    expect(tracker.process('\u001b]7;file://fic/var/log\u001b\\').paths, [
      '/var/log',
    ]);
  });

  test('decodes escaped characters in reported paths', () {
    final tracker = RemoteDirectoryTracker();
    expect(
      tracker.process('\u001b]7;file://fic/home/AR/My%20Files\u0007').paths,
      ['/home/AR/My Files'],
    );
  });

  test('quotes pasted directories as one shell argument', () {
    expect(quoteRemoteDirectory('/tmp/a b'), "'/tmp/a b'");
    expect(quoteRemoteDirectory("/tmp/user's"), "'/tmp/user'\"'\"'s'");
    expect(() => quoteRemoteDirectory('/tmp/a\nwhoami'), throwsFormatException);
    expect(remoteDirectoryShellArgument('~'), r'"$HOME"');
    expect(
      remoteDirectoryShellArgument('~/My Files'),
      r'"$HOME"/'
      "'My Files'",
    );
  });

  test('removes OSC reports while retaining surrounding terminal output', () {
    final tracker = RemoteDirectoryTracker();
    final update = tracker.process(
      'before\u001b]7;file://fic/home/AR\u0007after',
    );
    expect(update.paths, ['/home/AR']);
    expect(update.visibleOutput, 'beforeafter');
  });
}
