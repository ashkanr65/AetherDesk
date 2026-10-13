import 'package:ad_remote_desktop/native_bridge.dart';
import 'package:ad_remote_desktop/session_store.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('only the embedded engine enables Remote Desktop', () {
    final capabilities = CoreCapabilities.fromJson({
      'ssh': true,
      'sftp': true,
      'password': true,
      'sshKey': true,
      'ed25519': true,
      'remoteDesktop': false,
      'sshX11': true,
      'bundledX11': false,
    });

    expect(capabilities.remoteDesktop, isFalse);
    expect(capabilities.supportsRemoteDesktop, isFalse);
  });

  test('MATE and Cinnamon desktop choices survive profile persistence', () {
    for (final environment in const [
      DesktopEnvironment.mate,
      DesktopEnvironment.cinnamon,
    ]) {
      final profile = SavedSession(
        id: environment.name,
        name: environment.label,
        host: 'linux.example',
        port: 22,
        username: 'operator',
        desktopEnvironment: environment,
      );

      final decoded = SavedSession.fromJson(profile.toJson());
      expect(decoded.desktopEnvironment, environment);
    }
  });
}
