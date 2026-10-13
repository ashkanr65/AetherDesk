import 'dart:convert';
import 'dart:io';

import 'package:ad_remote_desktop/app_log.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory temp;

  setUp(() async {
    temp = await Directory.systemTemp.createTemp('aetherdesk-app-log-');
  });

  tearDown(() async {
    await temp.delete(recursive: true);
  });

  test('records persist as JSONL with level, source and context', () async {
    final log = AppLog.at(File('${temp.path}${Platform.pathSeparator}app.log'));
    log.session = 'session-1';
    log.profile = 'work';
    log.host = '10.0.0.5';
    log.error(
      AppLogSource.coreFfi,
      'SSH command failed',
      target: 'ad_ssh_read',
    );
    await log.recent();

    final line = (await File(log.path).readAsString()).trim();
    final json = Map<String, Object?>.from(jsonDecode(line) as Map);
    expect(json['level'], 'error');
    expect(json['source'], 'core.ffi');
    expect(json['msg'], 'SSH command failed');
    expect(json['target'], 'ad_ssh_read');
    expect(json['session'], 'session-1');
    expect(json['profile'], 'work');
    expect(json['host'], '10.0.0.5');
    expect(json['seq'], 1);
  });

  test('secrets and private keys are redacted but stack traces survive', () {
    expect(
      sanitizeLogText('login with password=hunter2 ok'),
      'login with password=<redacted> ok',
    );
    expect(sanitizeLogText('token: abc.def.ghi'), 'token: <redacted>');
    expect(
      sanitizeLogText(
        'key\n-----BEGIN PRIVATE KEY-----\nabc\n'
        '-----END PRIVATE KEY-----\ndone',
      ),
      'key\n<redacted private key>\ndone',
    );
    expect(
      sanitizeLogText('#0 NativeException.<anon>\n#1 NativeBridge.connect'),
      '#0 NativeException.<anon>\n#1 NativeBridge.connect',
    );
  });

  test('rotation shifts generations once the size cap is reached', () async {
    final log = AppLog.at(
      File('${temp.path}${Platform.pathSeparator}app.log'),
      maxBytes: 512,
      generations: 2,
    );
    for (var i = 0; i < 30; i++) {
      log.info(AppLogSource.uiDart, 'record $i ${'x' * 40}');
    }
    await log.recent();
    await log.rotateNow();

    expect(await File(log.path).exists(), isFalse);
    expect(await File('${log.path}.1').exists(), isTrue);
    expect(await File('${log.path}.2').exists(), isTrue);
  });

  test(
    'recent returns newest first and accepts Rust JSON and host fmt lines',
    () async {
      final log = AppLog.at(
        File('${temp.path}${Platform.pathSeparator}app.log'),
      );
      log.info(AppLogSource.uiDart, 'first');
      log.info(AppLogSource.uiDart, 'second');
      await log.recent();
      final records = await log.recent();
      expect(records.map((r) => r.message), ['second', 'first']);

      // Rust core (tracing-subscriber JSON) and host agent fmt lines parse too.
      final rust = AppLogRecord.tryParse(
        '{"ts":"2026-10-02T08:12:15.483Z","level":"WARN",'
        '"fields":{"message":"stream stalled"},"target":"ad_ssh::client"}',
      );
      expect(rust, isNotNull);
      expect(rust!.level, AppLogLevel.warn);
      expect(rust.message, 'stream stalled');
      expect(rust.target, 'ad_ssh::client');

      final host = AppLogRecord.tryParse(
        '2026-10-02T08:12:15.483Z ERROR agent_supervisor::compositor: boom',
      );
      expect(host, isNotNull);
      expect(host!.level, AppLogLevel.error);
      expect(host.source, AppLogSource.hostAgent);
      expect(host.message, 'boom');

      expect(AppLogRecord.tryParse('not a log line'), isNull);
      expect(AppLogRecord.tryParse('{broken json'), isNull);
    },
  );

  test('records keep error stacks and recent honours the limit', () async {
    final log = AppLog.at(File('${temp.path}${Platform.pathSeparator}app.log'));
    try {
      throw StateError('exploded');
    } on Object catch (error, stack) {
      log.error(AppLogSource.uiDart, 'failure', error: error, stack: stack);
    }
    log.info(AppLogSource.uiDart, 'tail');
    await log.recent();
    final records = await log.recent(limit: 1);
    expect(records, hasLength(1));
    expect(records.single.message, 'tail');

    final all = await log.recent();
    final failure = all.firstWhere((r) => r.message == 'failure');
    expect(failure.error, contains('exploded'));
    expect(failure.error, contains('app_log_test.dart'));
  });
  test('queued records retain their original session context', () async {
    final log = AppLog.at(File('${temp.path}/app.log'));
    log.session = 'first';
    log.info(AppLogSource.uiDart, 'first connection');
    log.session = 'second';
    log.info(AppLogSource.uiDart, 'second connection');
    final records = await log.recent();
    expect(records.first.session, 'second');
    expect(records.last.session, 'first');
  });

  test(
    'independent native, core and Dart files merge and redact on read',
    () async {
      final log = AppLog.at(
        File('${temp.path}/app-1-2.log'),
        mergeSources: true,
      );
      await File('${temp.path}/core-1.log').writeAsString(
        jsonEncode({
              'ts': '2026-10-02T08:12:15Z',
              'level': 'error',
              'source': 'core.rust',
              'msg': 'authorization: Bearer hidden-token',
            }) +
            '\n',
      );
      await File('${temp.path}/native-1.log.1').writeAsString(
        jsonEncode({
              'ts': '2026-10-02T08:12:16Z',
              'level': 'info',
              'source': 'ui.native',
              'msg': 'monitor changed',
            }) +
            '\n',
      );
      log.info(AppLogSource.uiDart, 'ready');
      final records = await log.recent();
      expect(records.map((r) => r.source).toSet(), {
        'ui.dart',
        'ui.native',
        'core.rust',
      });
      expect(
        records.map((r) => r.message).join(),
        isNot(contains('hidden-token')),
      );
    },
  );

  test('quoted credentials and truncated private keys are redacted', () {
    for (final value in [
      '{"password":"secret with spaces"}',
      'Authorization: Bearer secret-token',
      '-----BEGIN PRIVATE KEY-----\nsecret-key-without-end',
      'AETHERDESK_AGENT_V1 127.0.0.1 secret-endpoint',
    ]) {
      expect(sanitizeLogText(value), isNot(contains('secret')));
    }
  });

  test(
    'a burst is bounded instead of building an unlimited write queue',
    () async {
      final log = AppLog.at(File('${temp.path}/app.log'));
      for (var i = 0; i < 1000; i++) {
        log.info(AppLogSource.uiDart, 'burst $i');
      }
      final records = await log.recent(limit: 1000);
      expect(records.length, lessThanOrEqualTo(256));
      log.info(AppLogSource.uiDart, 'after burst');
      expect((await log.recent()).first.message, contains('Dropped'));
    },
  );
}
