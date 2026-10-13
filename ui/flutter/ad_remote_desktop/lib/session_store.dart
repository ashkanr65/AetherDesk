import 'dart:async';
import 'dart:convert';
import 'dart:io';

enum ConnectionType {
  sshTerminal,
  remoteDesktop,
  sshX11,
  sftpOnly;

  String get label => switch (this) {
    sshTerminal => 'SSH Terminal',
    remoteDesktop => 'Remote Desktop',
    sshX11 => 'SSH + X11',
    sftpOnly => 'SFTP Only',
  };

  String get tabSuffix => switch (this) {
    sshTerminal => 'SSH',
    remoteDesktop => 'Desktop',
    sshX11 => 'X11',
    sftpOnly => 'SFTP',
  };
}

enum AuthenticationMethod {
  password,
  sshKey,
  smartCard,
  mfa;

  String get label => switch (this) {
    password => 'Password',
    sshKey => 'SSH Key',
    smartCard => 'Smart Card / FIDO2',
    mfa => 'MFA / Keyboard Interactive',
  };
}

enum DesktopEnvironment {
  xfce,
  kde,
  gnome,
  mate,
  cinnamon,
  lxqt;

  String get label => switch (this) {
    xfce => 'XFCE',
    kde => 'KDE Plasma',
    gnome => 'GNOME',
    mate => 'MATE',
    cinnamon => 'Cinnamon',
    lxqt => 'LXQt',
  };
}

T _enumValue<T extends Enum>(List<T> values, Object? raw, T fallback) {
  final name = raw as String?;
  return values.where((value) => value.name == name).firstOrNull ?? fallback;
}

extension _FirstOrNull<T> on Iterable<T> {
  T? get firstOrNull {
    final iterator = this.iterator;
    return iterator.moveNext() ? iterator.current : null;
  }
}

class SavedSession {
  const SavedSession({
    required this.id,
    required this.name,
    required this.host,
    required this.port,
    required this.username,
    this.connectionType = ConnectionType.sshTerminal,
    this.authentication = AuthenticationMethod.password,
    this.desktopEnvironment = DesktopEnvironment.xfce,
    this.keyPath,
    this.wireguardConfigPath,
    this.rememberUsername = true,
    this.rememberPassword = false,
    this.masterPasswordProtected = false,
    this.favorite = false,
    this.deletedAt,
  });

  final String id;
  final String name;
  final String host;
  final int port;
  final String username;
  final ConnectionType connectionType;
  final AuthenticationMethod authentication;
  final DesktopEnvironment desktopEnvironment;
  final String? keyPath;
  final String? wireguardConfigPath;
  final bool rememberUsername;
  final bool rememberPassword;
  final bool masterPasswordProtected;
  final bool favorite;
  final DateTime? deletedAt;

  bool get isDeleted => deletedAt != null;
  bool get askEveryTime => !rememberPassword;

  SavedSession copyWith({
    String? id,
    String? name,
    String? host,
    int? port,
    String? username,
    ConnectionType? connectionType,
    AuthenticationMethod? authentication,
    DesktopEnvironment? desktopEnvironment,
    String? keyPath,
    bool clearKeyPath = false,
    String? wireguardConfigPath,
    bool clearWireguard = false,
    bool? rememberUsername,
    bool? rememberPassword,
    bool? masterPasswordProtected,
    bool? favorite,
    DateTime? deletedAt,
    bool restore = false,
  }) {
    return SavedSession(
      id: id ?? this.id,
      name: name ?? this.name,
      host: host ?? this.host,
      port: port ?? this.port,
      username: username ?? this.username,
      connectionType: connectionType ?? this.connectionType,
      authentication: authentication ?? this.authentication,
      desktopEnvironment: desktopEnvironment ?? this.desktopEnvironment,
      keyPath: clearKeyPath ? null : (keyPath ?? this.keyPath),
      wireguardConfigPath: clearWireguard
          ? null
          : (wireguardConfigPath ?? this.wireguardConfigPath),
      rememberUsername: rememberUsername ?? this.rememberUsername,
      rememberPassword: rememberPassword ?? this.rememberPassword,
      masterPasswordProtected:
          masterPasswordProtected ?? this.masterPasswordProtected,
      favorite: favorite ?? this.favorite,
      deletedAt: restore ? null : (deletedAt ?? this.deletedAt),
    );
  }

  Map<String, Object?> toJson() => {
    'version': 2,
    'id': id,
    'name': name,
    'host': host,
    'port': port,
    // Deliberately omit usernames when the user has not consented to save.
    'username': rememberUsername ? username : '',
    'connectionType': connectionType.name,
    'authentication': authentication.name,
    'desktopEnvironment': desktopEnvironment.name,
    if (keyPath != null && keyPath!.isNotEmpty) 'keyPath': keyPath,
    if (wireguardConfigPath != null) 'wireguardConfigPath': wireguardConfigPath,
    'rememberUsername': rememberUsername,
    'rememberPassword': rememberPassword,
    'masterPasswordProtected': rememberPassword && masterPasswordProtected,
    'favorite': favorite,
    'deletedAt': deletedAt?.toIso8601String(),
  };

  factory SavedSession.fromJson(Map<String, Object?> json) {
    final deleted = json['deletedAt'] as String?;
    final username = json['username'] as String? ?? '';
    return SavedSession(
      id: json['id'] as String,
      name: json['name'] as String,
      host: json['host'] as String,
      port: (json['port'] as num?)?.toInt() ?? 22,
      username: username,
      connectionType: _enumValue(
        ConnectionType.values,
        json['connectionType'],
        ConnectionType.sshTerminal,
      ),
      authentication: _enumValue(
        AuthenticationMethod.values,
        json['authentication'],
        AuthenticationMethod.password,
      ),
      desktopEnvironment: _enumValue(
        DesktopEnvironment.values,
        json['desktopEnvironment'],
        DesktopEnvironment.xfce,
      ),
      keyPath: json['keyPath'] as String?,
      wireguardConfigPath: json['wireguardConfigPath'] as String?,
      rememberUsername:
          json['rememberUsername'] as bool? ?? username.isNotEmpty,
      rememberPassword: json['rememberPassword'] as bool? ?? false,
      masterPasswordProtected:
          json['masterPasswordProtected'] as bool? ?? false,
      favorite: json['favorite'] as bool? ?? false,
      deletedAt: deleted == null ? null : DateTime.tryParse(deleted),
    );
  }
}

class SessionStore {
  SessionStore._(this._file);

  final File _file;
  Future<void> _pendingWrite = Future<void>.value();

  factory SessionStore.systemDefault() {
    final appData = Platform.environment['APPDATA'];
    final root = appData == null || appData.isEmpty
        ? Directory.current
        : Directory(appData);
    return SessionStore._(
      File(
        '${root.path}${Platform.pathSeparator}AetherDesk'
        '${Platform.pathSeparator}sessions.json',
      ),
    );
  }

  Future<List<SavedSession>> load() async {
    await _pendingWrite;
    final recovery = File('${_file.path}.bak');
    if (!await _file.exists() && await recovery.exists()) {
      await recovery.rename(_file.path);
    }
    if (!await _file.exists()) return <SavedSession>[];
    try {
      final decoded = jsonDecode(await _file.readAsString()) as List<dynamic>;
      return decoded
          .map(
            (item) =>
                SavedSession.fromJson(Map<String, Object?>.from(item as Map)),
          )
          .toList();
    } on Object {
      final backup = File('${_file.path}.corrupt');
      if (await backup.exists()) await backup.delete();
      await _file.rename(backup.path);
      return <SavedSession>[];
    }
  }

  Future<void> save(List<SavedSession> sessions) {
    final snapshot = List<SavedSession>.of(sessions);
    final operation = _pendingWrite.then((_) => _save(snapshot));
    _pendingWrite = operation.catchError((Object _) {});
    return operation;
  }

  Future<void> _save(List<SavedSession> sessions) async {
    await _file.parent.create(recursive: true);
    final temporary = File('${_file.path}.tmp');
    await temporary.writeAsString(
      const JsonEncoder.withIndent(
        '  ',
      ).convert(sessions.map((session) => session.toJson()).toList()),
      flush: true,
    );
    final backup = File('${_file.path}.bak');
    if (await backup.exists()) await backup.delete();
    if (await _file.exists()) await _file.rename(backup.path);
    try {
      await temporary.rename(_file.path);
    } on Object {
      if (!await _file.exists() && await backup.exists()) {
        await backup.rename(_file.path);
      }
      rethrow;
    }
    if (await backup.exists()) await backup.delete();
  }
}
