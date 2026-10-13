import 'package:ad_remote_desktop/terminal_output.dart';
import 'package:ad_remote_desktop/terminal_theme.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xterm2/xterm.dart';

void main() {
  final palette = TerminalPalettes.forPreset(TerminalThemePreset.mobaXterm);

  group('TerminalLogClassifier', () {
    const classifier = TerminalLogClassifier();
    final cases = <String, TerminalSemanticRole>{
      'fatal error: connection refused': TerminalSemanticRole.error,
      'WARNING: retrying operation': TerminalSemanticRole.warning,
      'build completed successfully': TerminalSemanticRole.success,
      'INFO service is ready': TerminalSemanticRole.information,
      'DEBUG packet trace': TerminalSemanticRole.debug,
      'connection established': TerminalSemanticRole.connection,
      'session disconnected': TerminalSemanticRole.disconnection,
      'SFTP upload completed': TerminalSemanticRole.fileTransfer,
      'host key fingerprint verified': TerminalSemanticRole.authentication,
      'systemd service changed state': TerminalSemanticRole.systemMessage,
      '**WARN: (IMPSYT-1507): display disabled': TerminalSemanticRole.warning,
      '**INFO: MMMC transition support': TerminalSemanticRole.information,
      '@file 142: # 7. INITIAL DESIGN LOAD': TerminalSemanticRole.systemMessage,
      '@file 147: puts "Reading MMMC..."': TerminalSemanticRole.debug,
    };

    for (final entry in cases.entries) {
      test('classifies ${entry.value.name}', () {
        expect(classifier.classify(entry.key), entry.value);
      });
    }
  });

  group('TerminalOutputParser', () {
    test('adds semantic true-color styling to complete plain lines', () {
      final parser = TerminalOutputParser();
      final output = parser.transform('ERROR: failed\r\n', palette);

      expect(output, startsWith('\x1b[1;38;2;'));
      expect(output, contains('ERROR: failed\x1b[0m\r\n'));
    });

    test('preserves remote ANSI sequences byte for byte', () {
      final parser = TerminalOutputParser();
      const remote = '\x1b[38;5;196merror\x1b[0m\r\n';
      expect(parser.transform(remote, palette), remote);
    });

    test('does not alter alternate-screen application output', () {
      final parser = TerminalOutputParser();
      parser.transform('\x1b[?1049h', palette);
      expect(parser.transform('fatal error\r\n', palette), 'fatal error\r\n');
      parser.transform('\x1b[?1049l', palette);
      expect(
        parser.transform('fatal error\r\n', palette),
        contains('\x1b[1;38;2;'),
      );
    });

    test('does not delay prompts or incomplete stream fragments', () {
      final parser = TerminalOutputParser();
      final output = parser.transform(r'[AR@fic ~]$ ', palette);
      expect(output, startsWith('\x1b[1;38;2;'));
      expect(output, contains(r'[AR@fic ~]$ '));
      expect(parser.transform('partial output', palette), 'partial output');
    });

    test('colors the Innovus prompt independently', () {
      final parser = TerminalOutputParser();
      final output = parser.transform('@innovus 4> ', palette);
      expect(output, startsWith('\x1b[1;38;2;'));
      expect(output, contains('@innovus 4> '));
    });

    test(
      'processes 100,000 streaming log lines without an unbounded buffer',
      () {
        final parser = TerminalOutputParser();
        final input =
            List.filled(100000, 'INFO service ready').join('\n') + '\n';
        final stopwatch = Stopwatch()..start();
        final output = parser.transform(input, palette);
        stopwatch.stop();

        expect('\n'.allMatches(output).length, 100000);
        expect(stopwatch.elapsed, lessThan(const Duration(seconds: 10)));
      },
    );
  });

  group('xterm color compatibility', () {
    test('keeps 256-color palette indices', () {
      final terminal = Terminal();
      terminal.write('\x1b[38;5;196mX');
      expect(
        terminal.buffer.lines[0].getForeground(0),
        CellColor.palette | 196,
      );
    });

    test('keeps 24-bit true-color values', () {
      final terminal = Terminal();
      terminal.write('\x1b[38;2;1;2;3mX');
      expect(
        terminal.buffer.lines[0].getForeground(0),
        CellColor.rgb | 0x010203,
      );
    });
  });
}
