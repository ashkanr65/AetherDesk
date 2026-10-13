import 'package:ad_remote_desktop/audit_log.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('sanitizeAuditDetail', () {
    test(
      'redacts secrets case-insensitively without an invalid regex flag',
      () {
        expect(
          sanitizeAuditDetail(
            'Password=hunter2 passphrase=alpha SECRET=beta result=failure',
          ),
          'Password=<redacted> passphrase=<redacted> SECRET=<redacted> '
          'result=failure',
        );
      },
    );

    test('flattens line breaks and limits retained detail', () {
      final longText = List.filled(700, 'x').join();
      final result = sanitizeAuditDetail('$longText\r\nnext');
      expect(result, hasLength(600));
      expect(result, isNot(contains('\n')));
    });
  });
}
