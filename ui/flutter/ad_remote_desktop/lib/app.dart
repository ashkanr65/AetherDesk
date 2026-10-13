import 'dart:async';
import 'package:flutter/gestures.dart';

import 'app_theme.dart';
import 'dart:convert';
import 'dart:ui' show AppExitResponse;
import 'package:flutter/rendering.dart';
import 'package:ad_stream_surface/ad_stream_surface.dart';
import 'assistant/screen_capture.dart';
import 'editor/editor_popup_host.dart';
import 'file_workspace/file_open_controller.dart';
import 'file_workspace/file_context_menu.dart';
import 'app_version.dart';

import 'package:flutter/material.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/services.dart';

import 'dart:io' show File, Platform, Process, ProcessStartMode;

import 'app_log.dart';
import 'assistant/assistant_page.dart';
import 'tools/tool_registry.dart';
import 'tools/tools_panel.dart';
import 'assistant/assistant_controller.dart';
import 'audit_log.dart';
import 'credential_vault.dart';
import 'native_bridge.dart';
import 'notifications.dart';
import 'remote_directory.dart';
import 'remote_desktop.dart';
import 'remote_desktop_view.dart';
import 'session_store.dart';
import 'session_tabs.dart';
import 'terminal_settings.dart';
import 'terminal_theme.dart';
import 'workspace.dart';
import 'x11_server.dart';

void runAetherDesk() => runApp(const AetherDeskDesktopApp());

class AetherDeskDesktopApp extends StatefulWidget {
  const AetherDeskDesktopApp({super.key});

  @override
  State<AetherDeskDesktopApp> createState() => _AetherDeskDesktopAppState();
}

class _AetherDeskDesktopAppState extends State<AetherDeskDesktopApp> {
  ThemeMode _themeMode = ThemeMode.dark;
  late final TerminalThemeManager _terminalThemeManager;

  @override
  void initState() {
    super.initState();
    _terminalThemeManager = TerminalThemeManager();
    unawaited(_terminalThemeManager.load());
  }

  @override
  void dispose() {
    unawaited(_terminalThemeManager.flush());
    _terminalThemeManager.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      title: 'AetherDesk',
      theme: aetherDeskTheme(Brightness.light),
      darkTheme: aetherDeskTheme(Brightness.dark),
      themeMode: _themeMode,
      home: AetherNoticeHost(
        child: AetherDeskHome(
          themeMode: _themeMode,
          terminalThemeManager: _terminalThemeManager,
          onToggleTheme: () => setState(
            () => _themeMode = _themeMode == ThemeMode.dark
                ? ThemeMode.light
                : ThemeMode.dark,
          ),
        ),
      ),
    );
  }
}

enum _TabState { connecting, connected, disconnected, failed }

class _ConnectionTab {
  _ConnectionTab({required this.id, required this.profile});

  final String id;
  final SavedSession profile;
  final workspaceKey = GlobalKey<ConnectedWorkspaceState>();
  String? sessionId;
  String? desktopSessionId;
  int connectionAttempt = 0;
  RemoteDesktopSession? remoteDesktop;
  String remoteDirectory = '.';
  _TabState state = _TabState.connecting;
  String? error;
}

class _ConnectionInput {
  const _ConnectionInput({required this.username, required this.secret});

  final String username;
  final String secret;
}

enum _SidebarSection {
  sessions('Sessions', Icons.terminal_rounded),
  bookmarks('Bookmarks', Icons.bookmark_outline_rounded),
  macros('Macros', Icons.bolt_outlined),
  tools('Tools', Icons.construction_outlined),
  connections('Connections', Icons.hub_outlined),
  monitoring('Monitoring', Icons.monitor_heart_outlined),
  logs('Logs', Icons.receipt_long_outlined),
  assistant('AI Assistant', Icons.auto_awesome_outlined);

  const _SidebarSection(this.label, this.icon);
  final String label;
  final IconData icon;
}

class AetherDeskHome extends StatefulWidget {
  const AetherDeskHome({
    super.key,
    required this.themeMode,
    required this.terminalThemeManager,
    required this.onToggleTheme,
    this.assistantControllerFactory,
  });

  final ThemeMode themeMode;
  final TerminalThemeManager terminalThemeManager;
  final VoidCallback onToggleTheme;
  final AssistantController Function(
    List<AssistantRemoteHost> Function() remoteHosts,
  )?
  assistantControllerFactory;

  @override
  State<AetherDeskHome> createState() => _AetherDeskHomeState();
}

class _AetherDeskHomeState extends State<AetherDeskHome> {
  late final AppLifecycleListener _exitListener;
  Future<AppExitResponse>? _exitRequest;
  Future<AppExitResponse> _prepareExit() async {
    if (_assistantLocalActions != null &&
        !await _assistantLocalActions!.prepareToClose()) {
      return AppExitResponse.cancel;
    }
    await _assistantLocalEditor?.closeForDisconnect();
    for (final tab in List<_ConnectionTab>.of(_tabs)) {
      if (!(await tab.workspaceKey.currentState?.prepareToClose() ?? true))
        return AppExitResponse.cancel;
    }
    for (final tab in List<_ConnectionTab>.of(_tabs)) {
      if (!(await tab.workspaceKey.currentState?.finishClosingEditor() ??
          true)) {
        return AppExitResponse.cancel;
      }
    }
    final assistant = _assistant;
    if (assistant != null) await _shutdownAssistantSafely(assistant);
    try {
      await _toolsPanelKey.currentState?.shutdown();
    } on Object {
      debugPrint('Tool cleanup could not finish during application exit.');
    }
    // Deterministic host cleanup: shells, SFTP subsystems and forwarded
    // channels must not outlive the client. Remote desktops detach so the
    // next connection resumes the same host session.
    for (final tab in List<_ConnectionTab>.of(_tabs)) {
      final remoteDesktop = tab.remoteDesktop;
      if (remoteDesktop != null) {
        try {
          await remoteDesktop.dispose().timeout(const Duration(seconds: 8));
        } on Object {
          debugPrint(
            'Remote desktop cleanup did not finish during application exit.',
          );
        }
      }
      final sessionId = tab.sessionId;
      if (sessionId != null) {
        try {
          await NativeBridge.disconnect(
            sessionId,
          ).timeout(const Duration(seconds: 8));
        } on Object {
          debugPrint('SSH cleanup did not finish during application exit.');
        }
      }
    }
    try {
      await NativeBridge.shutdownAll();
    } on Object {
      debugPrint('Final connection cleanup failed during application exit.');
    }
    return AppExitResponse.exit;
  }

  final SessionStore _store = SessionStore.systemDefault();
  final CredentialVault _vault = const CredentialVault();
  final AuditLog _audit = AuditLog.systemDefault();
  final X11ServerManager _x11Server = X11ServerManager();
  final RemoteDesktopLauncher _remoteDesktopLauncher = RemoteDesktopLauncher();
  final TextEditingController _searchController = TextEditingController();
  final TextEditingController _remotePathController = TextEditingController();
  final SessionTabCollection<_ConnectionTab> _tabCollection =
      SessionTabCollection<_ConnectionTab>();
  List<SavedSession> _sessions = [];
  List<AuditEntry> _auditEntries = const [];
  int _logsTab = 0;
  List<AppLogRecord> _appLogRecords = const [];
  List<AppLogRecord> _hostLogRecords = const [];
  bool _hostLogBusy = false;
  bool _hostLogFetching = false;
  String? _hostLogNote;
  String? _hostDesktopId;
  final Set<AppLogLevel> _logLevelFilters = AppLogLevel.values.toSet();
  String? _logSourceFilter;
  String _logQuery = '';
  bool _logCaseSensitive = false;
  bool _logFollow = true;
  Timer? _hostLogTimer;
  CoreCapabilities? _capabilities;
  String? _nativeError;
  bool _nativeErrorNoticeVisible = false;
  String? _selectedTabId;
  String _search = '';
  bool _loading = true;
  bool _sidebarVisible = true;
  double _sidebarWidth = 314;
  double _sidebarDragStartX = 0, _sidebarDragStartWidth = 314;
  final _connectionCaptureKey = GlobalKey();
  FileWorkspaceActions? _assistantLocalActions;
  EditorPopupHost? _assistantLocalEditor;
  bool _auditWarningShown = false;
  bool _checkingX11Health = false;
  Timer? _x11HealthTimer;
  _SidebarSection _sidebarSection = _SidebarSection.sessions;
  AssistantController? _assistant;
  bool _toolsOpened = false;
  final _toolsPanelKey = GlobalKey<ToolsPanelState>();
  bool _assistantShutdownWarningRecorded = false;

  List<AssistantRemoteHost> _assistantRemoteHosts() => [
    for (final tab in _tabs)
      if (tab.state == _TabState.connected &&
          tab.sessionId != null &&
          tab.profile.connectionType != ConnectionType.remoteDesktop)
        AssistantRemoteHost(
          id: tab.sessionId!,
          identity: jsonEncode([
            tab.profile.id,
            tab.profile.host,
            tab.profile.port,
            tab.profile.username,
          ]),
          label:
              '${tab.profile.name} (${tab.profile.username}@${tab.profile.host}:${tab.profile.port})',
          directory: tab.remoteDirectory,
        ),
  ];

  List<_ConnectionTab> get _tabs => _tabCollection.items;

  _ConnectionTab? get _selectedTab {
    for (final tab in _tabs) {
      if (tab.id == _selectedTabId) return tab;
    }
    return null;
  }

  @override
  void initState() {
    super.initState();
    _exitListener = AppLifecycleListener(
      onExitRequested: () async {
        if (_exitRequest != null) return _exitRequest!;
        _exitRequest = _prepareExit();
        try {
          return await _exitRequest!;
        } finally {
          _exitRequest = null;
        }
      },
    );
    try {
      final bridge = NativeBridge();
      bridge.versionSync();
      _capabilities = bridge.capabilitiesSync();
    } on Object catch (error) {
      _nativeError = error.toString();
      _nativeErrorNoticeVisible = true;
    }
    _x11HealthTimer = Timer.periodic(
      const Duration(seconds: 3),
      (_) => unawaited(_checkX11Health()),
    );
    _loadSessions();
  }

  List<_ConnectionTab> get _activeConnections => activeConnectionTabs(
    _tabs,
    (tab) =>
        tab.state == _TabState.connecting || tab.state == _TabState.connected,
  );

  bool _hasRemoteShell(_ConnectionTab? tab) =>
      tab != null &&
      (tab.profile.connectionType == ConnectionType.sshTerminal ||
          tab.profile.connectionType == ConnectionType.sshX11);

  void _syncRemotePathField() {
    final tab = _selectedTab;
    final text =
        tab != null && tab.state == _TabState.connected && _hasRemoteShell(tab)
        ? tab.remoteDirectory
        : '';
    if (_remotePathController.text == text) return;
    _remotePathController.value = TextEditingValue(
      text: text,
      selection: TextSelection.collapsed(offset: text.length),
    );
  }

  void _selectConnectionTab(_ConnectionTab tab) {
    setState(() => _selectedTabId = tab.id);
    _syncRemotePathField();
  }

  void _handleRemoteDirectoryChanged(_ConnectionTab tab, String path) {
    if (!_tabs.contains(tab) || path == tab.remoteDirectory) return;
    tab.remoteDirectory = path;
    if (_selectedTabId == tab.id) _syncRemotePathField();
  }

  void _changeRemoteDirectory(String path) {
    final tab = _selectedTab;
    final sessionId = tab?.sessionId;
    if (tab == null ||
        sessionId == null ||
        tab.state != _TabState.connected ||
        !_hasRemoteShell(tab)) {
      _showMessage(
        'Select an active SSH terminal before changing its directory.',
        kind: NoticeKind.warning,
      );
      _syncRemotePathField();
      return;
    }
    if (path.isEmpty) {
      _syncRemotePathField();
      return;
    }
    try {
      final argument = remoteDirectoryShellArgument(path);
      NativeBridge().write(sessionId, 'cd -- $argument\r');
    } on Object catch (error) {
      _showMessage('Could not change the remote directory: $error');
      _syncRemotePathField();
    }
  }

  Future<void> _copyRemoteDirectory() async {
    final path = _remotePathController.text;
    if (path.isEmpty) return;
    await Clipboard.setData(ClipboardData(text: path));
    _showMessage('Remote directory copied.', kind: NoticeKind.information);
  }

  Future<void> _loadSessions() async {
    try {
      final sessions = await _store.load();
      if (!mounted) return;
      setState(() => _sessions = sessions);
    } on Object catch (error) {
      _showMessage('Could not load saved sessions: $error');
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _persist() async {
    try {
      await _store.save(_sessions);
    } on Object catch (error) {
      _showMessage('Could not save sessions: $error');
    }
  }

  Iterable<SavedSession> get _visibleSessions {
    final query = _search.trim().toLowerCase();
    final visible =
        _sessions.where((session) {
          if (session.isDeleted) return false;
          if (query.isEmpty) return true;
          return session.name.toLowerCase().contains(query) ||
              session.host.toLowerCase().contains(query) ||
              session.username.toLowerCase().contains(query);
        }).toList()..sort((left, right) {
          if (left.favorite != right.favorite) return left.favorite ? -1 : 1;
          return left.name.toLowerCase().compareTo(right.name.toLowerCase());
        });
    return visible;
  }

  Future<void> _showSessionEditor({
    SavedSession? existing,
    bool connect = true,
  }) async {
    CredentialMetadata? metadata;
    if (existing != null && existing.rememberPassword) {
      try {
        metadata = await _vault.inspect(existing.id);
      } on Object catch (error) {
        _showMessage(
          'Credential vault warning: $error',
          kind: NoticeKind.warning,
        );
      }
    }
    if (!mounted) return;
    final request = await showDialog<_SessionRequest>(
      context: context,
      barrierDismissible: false,
      builder: (context) => _SessionDialog(
        existing: existing,
        connectByDefault: connect,
        capabilities: _capabilities,
        storedCredential: metadata,
      ),
    );
    if (request == null) return;
    var profile = request.profile;

    if (request.save) {
      try {
        if (profile.rememberPassword) {
          if (request.secret.isNotEmpty) {
            await _vault.store(
              profileId: profile.id,
              username: profile.rememberUsername ? profile.username : '',
              secret: request.secret,
              masterPassword: request.masterPassword,
            );
            await _recordAudit(
              event: 'credential.store',
              result: 'success',
              profileId: profile.id,
              profileName: profile.name,
              host: profile.host,
              detail: profile.masterPasswordProtected
                  ? 'Windows Credential Manager + DPAPI + master password'
                  : 'Windows Credential Manager (DPAPI layer where available)',
            );
          } else if (metadata == null) {
            _showMessage(
              'Enter a secret before choosing Remember Password.',
              kind: NoticeKind.warning,
            );
            return;
          } else {
            await _vault.updateUsername(
              profile.id,
              profile.rememberUsername ? profile.username : '',
            );
            await _recordAudit(
              event: 'credential.username-policy',
              result: 'success',
              profileId: profile.id,
              profileName: profile.name,
              host: profile.host,
              detail: profile.rememberUsername
                  ? 'Username retained in Windows Credential Manager'
                  : 'Username removed from Windows Credential Manager',
            );
          }
        } else if (metadata != null) {
          await _vault.delete(profile.id);
          await _recordAudit(
            event: 'credential.delete',
            result: 'success',
            profileId: profile.id,
            profileName: profile.name,
            host: profile.host,
          );
        }
      } on Object catch (error) {
        await _recordAudit(
          event: 'credential.store',
          result: 'failure',
          profileId: profile.id,
          profileName: profile.name,
          host: profile.host,
          detail: error.toString(),
        );
        if (metadata == null &&
            profile.rememberPassword &&
            request.secret.isNotEmpty) {
          profile = profile.copyWith(
            rememberPassword: false,
            masterPasswordProtected: false,
          );
          _showMessage(
            'Credential storage is unavailable. The profile was saved as '
            'Ask Every Time and this connection will continue.',
            kind: NoticeKind.warning,
          );
        } else {
          _showMessage('Credential was not saved: $error');
          return;
        }
      }

      final index = _sessions.indexWhere((item) => item.id == profile.id);
      setState(() {
        if (index < 0) {
          _sessions.add(profile);
        } else {
          _sessions[index] = profile;
        }
      });
      await _persist();
    }

    if (request.connect) {
      final input = request.secret.isNotEmpty && profile.username.isNotEmpty
          ? _ConnectionInput(username: profile.username, secret: request.secret)
          : await _resolveConnectionInput(profile);
      if (input != null) {
        await _openConnection(profile, input);
      }
    }
  }

  Future<_ConnectionInput?> _resolveConnectionInput(
    SavedSession profile,
  ) async {
    var username = profile.username;
    String? storedSecret;
    if (profile.rememberPassword) {
      try {
        final metadata = await _vault.inspect(profile.id);
        if (metadata != null) {
          String? masterPassword;
          if (metadata.masterPasswordProtected) {
            masterPassword = await _promptForMasterPassword(profile.name);
            if (masterPassword == null) return null;
          }
          final credential = await _vault.read(
            profile.id,
            masterPassword: masterPassword,
          );
          if (credential != null) {
            username = username.isEmpty ? credential.username : username;
            storedSecret = credential.secret;
            if (username.isNotEmpty) {
              return _ConnectionInput(username: username, secret: storedSecret);
            }
          }
        }
      } on InvalidMasterPasswordException catch (error) {
        _showMessage(error.toString());
        return null;
      } on Object catch (error) {
        _showMessage('Could not unlock credential: $error');
      }
    }

    if (!mounted) return null;
    return showDialog<_ConnectionInput>(
      context: context,
      builder: (context) => _CredentialPrompt(
        profile: profile,
        initialUsername: username,
        storedSecret: storedSecret,
      ),
    );
  }

  Future<String?> _promptForMasterPassword(String profileName) async {
    final controller = TextEditingController();
    var hidden = true;
    final result = await showDialog<String>(
      context: context,
      barrierDismissible: false,
      builder: (context) => StatefulBuilder(
        builder: (context, setDialogState) => AlertDialog(
          title: const Text('Unlock credential'),
          content: SizedBox(
            width: 420,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('Enter the master password for “$profileName”.'),
                const SizedBox(height: 14),
                TextField(
                  controller: controller,
                  autofocus: true,
                  obscureText: hidden,
                  onSubmitted: (value) {
                    if (value.isNotEmpty) Navigator.pop(context, value);
                  },
                  decoration: InputDecoration(
                    labelText: 'Master password',
                    suffixIcon: IconButton(
                      onPressed: () => setDialogState(() => hidden = !hidden),
                      icon: Icon(
                        hidden
                            ? Icons.visibility_outlined
                            : Icons.visibility_off_outlined,
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () {
                if (controller.text.isNotEmpty) {
                  Navigator.pop(context, controller.text);
                }
              },
              child: const Text('Unlock'),
            ),
          ],
        ),
      ),
    );
    controller.clear();
    controller.dispose();
    return result;
  }

  bool _isConnectionTypeAvailable(ConnectionType type) => switch (type) {
    ConnectionType.sshTerminal => _capabilities?.ssh ?? false,
    ConnectionType.sftpOnly => _capabilities?.sftp ?? false,
    ConnectionType.remoteDesktop =>
      _capabilities?.supportsRemoteDesktop ?? false,
    ConnectionType.sshX11 => _capabilities?.sshX11 ?? false,
  };

  bool _isAuthenticationAvailable(AuthenticationMethod method) =>
      switch (method) {
        AuthenticationMethod.password => _capabilities?.password ?? false,
        AuthenticationMethod.sshKey => _capabilities?.sshKey ?? false,
        AuthenticationMethod.smartCard =>
          (_capabilities?.smartCard ?? false) ||
              (_capabilities?.fido2 ?? false),
        AuthenticationMethod.mfa => _capabilities?.mfa ?? false,
      };

  Future<void> _openConnection(
    SavedSession profile,
    _ConnectionInput input,
  ) async {
    if (_nativeError != null) {
      _showMessage('Native core unavailable: $_nativeError');
      return;
    }
    if (!_isConnectionTypeAvailable(profile.connectionType)) {
      _showUnavailableCapability(profile.connectionType.label);
      return;
    }
    if (!_isAuthenticationAvailable(profile.authentication)) {
      _showUnavailableCapability(profile.authentication.label);
      return;
    }

    final tab = _ConnectionTab(
      id: DateTime.now().microsecondsSinceEpoch.toString(),
      profile: profile.copyWith(username: input.username),
    );
    setState(() {
      _tabCollection.openNew(tab);
      _selectedTabId = tab.id;
      _sidebarSection = _SidebarSection.connections;
    });
    _syncRemotePathField();
    await _establish(tab, input);
  }

  Future<void> _establish(
    _ConnectionTab tab,
    _ConnectionInput input, {
    bool trustNewHostKey = false,
  }) async {
    if (!_tabs.contains(tab)) return;
    final attempt = ++tab.connectionAttempt;
    setState(() {
      tab
        ..state = _TabState.connecting
        ..error = null
        ..sessionId = null
        ..remoteDirectory = '.';
    });
    _syncRemotePathField();
    String? openedSessionId;
    RemoteDesktopSession? remoteDesktop;
    try {
      await _recordAudit(
        event: 'connection.start',
        result: 'attempt',
        profileId: tab.profile.id,
        profileName: tab.profile.name,
        host: tab.profile.host,
        detail:
            '${tab.profile.connectionType.name}/${tab.profile.authentication.name}',
      );
      X11ConnectionOptions? x11;
      if (tab.profile.connectionType == ConnectionType.sshX11) {
        x11 = (await _x11Server.ensureRunning()).options;
      }
      if (!mounted || !_tabs.contains(tab) || tab.connectionAttempt != attempt)
        return;
      final sessionId = await NativeBridge.connect(
        tab.profile.host,
        tab.profile.port,
        input.username,
        input.secret,
        authentication: tab.profile.authentication.name,
        keyPath: tab.profile.keyPath ?? '',
        wireguardConfigPath: tab.profile.wireguardConfigPath ?? '',
        trustNewHostKey: trustNewHostKey,
        openShell:
            tab.profile.connectionType == ConnectionType.sshTerminal ||
            tab.profile.connectionType == ConnectionType.sshX11,
        x11: x11,
      );
      openedSessionId = sessionId;
      if (tab.profile.connectionType == ConnectionType.remoteDesktop) {
        final readinessOutput = await NativeBridge.executeCommand(
          sessionId,
          RemoteDesktopLauncher.readinessCommand(
            tab.profile.desktopEnvironment,
          ),
        );
        final readiness = RemoteDesktopReadiness.parse(readinessOutput);
        await _recordAudit(
          event: 'remote.probe',
          result: readiness.selectedBackend == RemoteDesktopBackend.unavailable
              ? 'failure'
              : 'success',
          profileId: tab.profile.id,
          profileName: tab.profile.name,
          host: tab.profile.host,
          detail: readiness.diagnosticSummary,
        );
        if (!readiness.desktopReady) {
          throw RemoteDesktopException(
            '${tab.profile.desktopEnvironment.label} is not installed on '
            'the Linux host (missing '
            '${readiness.missingDesktopCommand ?? 'desktop launcher'}).',
          );
        }
        if (!readiness.linuxReady) {
          throw const RemoteDesktopException(
            'Aether Stream requires a Linux x86-64 host.',
          );
        }
        if (!readiness.dbusReady) {
          throw const RemoteDesktopException(
            'The remote desktop needs dbus-run-session on the Linux host.',
          );
        }
        // The Logs panel fetches this session's host-side agent.log.
        tab.desktopSessionId = persistentDesktopId(tab.profile, input.username);
        remoteDesktop = await _remoteDesktopLauncher.launch(
          sshSessionId: sessionId,
          profile: tab.profile,
          username: input.username,
          sshSecret: input.secret,
        );
      }
      if (!mounted ||
          !_tabs.contains(tab) ||
          tab.connectionAttempt != attempt) {
        await remoteDesktop?.dispose();
        await NativeBridge.disconnect(sessionId);
        return;
      }
      setState(() {
        tab
          ..sessionId = sessionId
          ..remoteDesktop = remoteDesktop
          ..state = _TabState.connected
          ..error = null;
      });
      _syncRemotePathField();
      await _recordAudit(
        event: 'connection.open',
        result: 'success',
        profileId: tab.profile.id,
        profileName: tab.profile.name,
        host: tab.profile.host,
      );
      if (remoteDesktop != null) {
        unawaited(
          remoteDesktop.closed.then(
            (_) => _handleRemoteDesktopExit(tab, remoteDesktop!),
          ),
        );
      }
    } on UnknownHostKeyException catch (hostKey) {
      if (!mounted || !_tabs.contains(tab) || tab.connectionAttempt != attempt)
        return;
      final trusted = await _confirmHostKey(hostKey);
      if (!mounted || !_tabs.contains(tab) || tab.connectionAttempt != attempt)
        return;
      if (trusted) {
        await _recordAudit(
          event: 'host_key.accept',
          result: 'confirmed',
          profileId: tab.profile.id,
          profileName: tab.profile.name,
          host: tab.profile.host,
          detail: '${hostKey.algorithm} ${hostKey.fingerprint}',
        );
        await _establish(tab, input, trustNewHostKey: true);
      } else {
        await _failTab(tab, 'Host fingerprint was not trusted.');
      }
    } on Object catch (error) {
      await remoteDesktop?.dispose();
      if (openedSessionId != null) {
        await _disconnectNative(openedSessionId, profile: tab.profile);
      }
      if (tab.connectionAttempt == attempt)
        await _failTab(tab, error.toString());
    } finally {
      await _stopUnusedX11Server();
    }
  }

  Future<void> _handleRemoteDesktopExit(
    _ConnectionTab tab,
    RemoteDesktopSession remoteDesktop,
  ) async {
    if (!mounted ||
        !_tabs.contains(tab) ||
        tab.remoteDesktop != remoteDesktop) {
      return;
    }
    final sessionId = tab.sessionId;
    final desktopError = remoteDesktop.status.value.error;
    await remoteDesktop.dispose();
    if (!mounted || !_tabs.contains(tab)) return;
    setState(() {
      tab
        ..remoteDesktop = null
        ..sessionId = null
        ..state = desktopError == null
            ? _TabState.disconnected
            : _TabState.failed
        ..error = desktopError;
    });
    _syncRemotePathField();
    if (desktopError == null) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Remote desktop disconnected successfully.'),
        ),
      );
    }
    if (sessionId != null) {
      await _disconnectNative(sessionId, profile: tab.profile);
    }
  }

  Future<bool> _confirmHostKey(UnknownHostKeyException hostKey) async {
    if (!mounted) return false;
    return await showDialog<bool>(
          context: context,
          barrierDismissible: false,
          builder: (context) => AlertDialog(
            icon: const Icon(Icons.verified_user_outlined, size: 34),
            title: const Text('Verify server identity'),
            content: SizedBox(
              width: 520,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'AetherDesk has not connected to '
                    '${hostKey.host}:${hostKey.port} before.',
                  ),
                  const SizedBox(height: 16),
                  const Text(
                    'Compare this fingerprint with one supplied by your server administrator:',
                  ),
                  const SizedBox(height: 10),
                  SelectableText(
                    hostKey.fingerprint,
                    style: const TextStyle(
                      fontFamily: 'monospace',
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    hostKey.algorithm,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(height: 16),
                  const Text(
                    'Decline if you cannot verify it. Accepting an unexpected key can expose the session to a man-in-the-middle attack.',
                  ),
                ],
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(context, false),
                child: const Text('Cancel'),
              ),
              FilledButton(
                onPressed: () => Navigator.pop(context, true),
                child: const Text('Trust this fingerprint'),
              ),
            ],
          ),
        ) ??
        false;
  }

  Future<void> _failTab(_ConnectionTab tab, String error) async {
    if (!mounted || !_tabs.contains(tab)) return;
    setState(() {
      tab
        ..state = _TabState.failed
        ..error = error
        ..sessionId = null;
    });
    _syncRemotePathField();
    await _recordAudit(
      event: 'connection.open',
      result: 'failure',
      profileId: tab.profile.id,
      profileName: tab.profile.name,
      host: tab.profile.host,
      detail: error,
    );
  }

  Future<void> _openSaved(SavedSession profile) async {
    setState(() => _sidebarSection = _SidebarSection.connections);
    _syncRemotePathField();
    final input = await _resolveConnectionInput(profile);
    if (input == null) return;
    await _openConnection(profile, input);
  }

  Future<void> _reconnectTab(_ConnectionTab tab) async {
    final input = await _resolveConnectionInput(tab.profile);
    if (input == null) return;
    final sessionId = tab.sessionId;
    final remoteDesktop = tab.remoteDesktop;
    tab.remoteDesktop = null;
    await remoteDesktop?.dispose();
    if (sessionId != null) {
      await _disconnectNative(sessionId, profile: tab.profile);
    }
    await _establish(tab, input);
  }

  Future<void> _handleConnectionLost(_ConnectionTab tab, String error) async {
    if (!mounted || !_tabs.contains(tab) || tab.state != _TabState.connected) {
      return;
    }
    final sessionId = tab.sessionId;
    setState(() {
      tab
        ..sessionId = null
        ..remoteDesktop = null
        ..state = _TabState.disconnected
        ..error = error;
    });
    _syncRemotePathField();
    await _recordAudit(
      event: 'connection.lost',
      result: 'failure',
      profileId: tab.profile.id,
      profileName: tab.profile.name,
      host: tab.profile.host,
      detail: error,
    );
    if (sessionId != null) {
      await _disconnectNative(sessionId, profile: tab.profile);
    }
  }

  Future<void> _checkX11Health() async {
    if (_checkingX11Health || !mounted) return;
    final x11Tabs = _tabs
        .where(
          (tab) =>
              tab.state == _TabState.connected &&
              tab.profile.connectionType == ConnectionType.sshX11,
        )
        .toList();
    if (x11Tabs.isEmpty) return;
    _checkingX11Health = true;
    try {
      if (await _x11Server.isRunning() || !mounted) return;
      final exitCode = _x11Server.exitCode;
      final error =
          'The local X11 display stopped'
          '${exitCode == null ? '' : ' (exit code $exitCode)'}. '
          'Reconnect to negotiate a new secure DISPLAY.';
      final sessions = <(_ConnectionTab, String)>[];
      setState(() {
        for (final tab in x11Tabs) {
          if (!_tabs.contains(tab) || tab.state != _TabState.connected) {
            continue;
          }
          final sessionId = tab.sessionId;
          if (sessionId != null) sessions.add((tab, sessionId));
          tab
            ..sessionId = null
            ..state = _TabState.disconnected
            ..error = error;
        }
      });
      _syncRemotePathField();
      for (final (tab, sessionId) in sessions) {
        await _recordAudit(
          event: 'connection.lost',
          result: 'failure',
          profileId: tab.profile.id,
          profileName: tab.profile.name,
          host: tab.profile.host,
          detail: error,
        );
        await _disconnectNative(sessionId, profile: tab.profile);
      }
      await _x11Server.stop();
      _showMessage(
        'The local X11 display restarted or became unavailable. Existing X11 sessions were closed safely; reconnect them to receive a new DISPLAY.',
        kind: NoticeKind.warning,
      );
    } finally {
      _checkingX11Health = false;
    }
  }

  Future<void> _disconnectTab(_ConnectionTab tab) async {
    if (!(await tab.workspaceKey.currentState?.prepareToClose() ?? true))
      return;
    if (!(await tab.workspaceKey.currentState?.finishClosingEditor() ?? true))
      return;
    if (!mounted) return;
    tab.connectionAttempt++;
    final sessionId = tab.sessionId;
    final remoteDesktop = tab.remoteDesktop;
    setState(() {
      tab
        ..remoteDesktop = null
        ..sessionId = null
        ..state = _TabState.disconnected
        ..error = null;
    });
    _syncRemotePathField();
    // Disconnecting ends the host desktop so the next connection starts a new
    // remote session; closing only the tab keeps it resumable instead.
    await remoteDesktop?.dispose(terminate: true);
    if (sessionId != null) {
      await _disconnectNative(sessionId, profile: tab.profile);
    }
    await _stopUnusedX11Server();
  }

  Future<void> _stopUnusedX11Server() async {
    if (_tabs.any(
      (tab) =>
          tab.profile.connectionType == ConnectionType.sshX11 &&
          (tab.state == _TabState.connecting ||
              tab.state == _TabState.connected),
    ))
      return;
    await _x11Server.stop();
  }

  Future<void> _disconnectNative(
    String sessionId, {
    required SavedSession profile,
  }) async {
    try {
      await NativeBridge.disconnect(sessionId);
      await _recordAudit(
        event: 'connection.close',
        result: 'success',
        profileId: profile.id,
        profileName: profile.name,
        host: profile.host,
      );
    } on Object catch (error) {
      await _recordAudit(
        event: 'connection.close',
        result: 'warning',
        profileId: profile.id,
        profileName: profile.name,
        host: profile.host,
        detail: error.toString(),
      );
    } finally {
      await _stopUnusedX11Server();
    }
  }

  Future<void> _closeTab(_ConnectionTab tab) async {
    if (!(await tab.workspaceKey.currentState?.prepareToClose() ?? true))
      return;
    if (!(await tab.workspaceKey.currentState?.finishClosingEditor() ?? true))
      return;
    if (!mounted) return;
    tab.connectionAttempt++;
    final index = _tabs.indexOf(tab);
    if (index < 0) return;
    final sessionId = tab.sessionId;
    final remoteDesktop = tab.remoteDesktop;
    setState(() {
      _tabs.removeAt(index);
      if (_selectedTabId == tab.id) {
        if (_tabs.isEmpty) {
          _selectedTabId = null;
        } else {
          _selectedTabId = _tabs[index.clamp(0, _tabs.length - 1)].id;
        }
      }
    });
    _syncRemotePathField();
    await remoteDesktop?.dispose();
    if (sessionId != null) {
      await _disconnectNative(sessionId, profile: tab.profile);
    }
    await _stopUnusedX11Server();
  }

  void _selectRelativeTab(int delta) {
    if (_tabs.isEmpty) return;
    final currentIndex = _tabs.indexWhere((tab) => tab.id == _selectedTabId);
    final nextIndex = wrappedTabIndex(
      currentIndex: currentIndex < 0 ? 0 : currentIndex,
      tabCount: _tabs.length,
      delta: delta,
    );
    if (nextIndex >= 0) {
      setState(() => _selectedTabId = _tabs[nextIndex].id);
      _syncRemotePathField();
    }
  }

  Future<void> _sessionAction(String action, SavedSession session) async {
    switch (action) {
      case 'open':
        await _openSaved(session);
        break;
      case 'edit':
        await _showSessionEditor(existing: session, connect: false);
        break;
      case 'rename':
        final name = await _promptForName('Rename session', session.name);
        if (name == null || name.trim().isEmpty) return;
        await _replaceSession(session.copyWith(name: name.trim()));
        break;
      case 'duplicate':
        final copy = session.copyWith(
          id: DateTime.now().microsecondsSinceEpoch.toString(),
          name: '${session.name} copy',
          rememberPassword: false,
          masterPasswordProtected: false,
          favorite: false,
          restore: true,
        );
        setState(() => _sessions.add(copy));
        await _persist();
        break;
      case 'favorite':
        await _replaceSession(session.copyWith(favorite: !session.favorite));
        break;
      case 'forgetCredential':
        await _vault.delete(session.id);
        await _replaceSession(
          session.copyWith(
            rememberPassword: false,
            masterPasswordProtected: false,
          ),
        );
        await _recordAudit(
          event: 'credential.delete',
          result: 'success',
          profileId: session.id,
          profileName: session.name,
          host: session.host,
        );
        _showMessage(
          'Stored credential removed.',
          kind: NoticeKind.information,
        );
        break;
      case 'delete':
        await _deleteSession(session);
        break;
    }
  }

  Future<void> _replaceSession(SavedSession replacement) async {
    final index = _sessions.indexWhere((item) => item.id == replacement.id);
    if (index < 0) return;
    // Open tabs are immutable connection snapshots. Editing or deleting the
    // saved profile must not change the mode, host, or identity of a running
    // session that was created from it.
    setState(() => _sessions[index] = replacement);
    await _persist();
  }

  Future<void> _deleteSession(SavedSession session) async {
    final confirmed =
        await showDialog<bool>(
          context: context,
          builder: (context) => AlertDialog(
            title: const Text('Delete connection?'),
            content: Text(
              '“${session.name}” will move to deleted connections. '
              'Any stored credential will be removed immediately.',
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(context, false),
                child: const Text('Cancel'),
              ),
              FilledButton(
                onPressed: () => Navigator.pop(context, true),
                child: const Text('Delete'),
              ),
            ],
          ),
        ) ??
        false;
    if (!confirmed) return;
    await _vault.delete(session.id);
    final deleted = session.copyWith(
      deletedAt: DateTime.now(),
      rememberPassword: false,
      masterPasswordProtected: false,
    );
    await _replaceSession(deleted);
    if (!mounted) return;
    AetherNotice.show(
      context,
      message: 'Connection deleted: ${session.name}',
      kind: NoticeKind.information,
      action: SnackBarAction(
        label: 'Undo profile',
        onPressed: () => _replaceSession(deleted.copyWith(restore: true)),
      ),
    );
  }

  Future<String?> _promptForName(String title, String initial) async {
    final controller = TextEditingController(text: initial);
    final result = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title),
        content: TextField(controller: controller, autofocus: true),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, controller.text),
            child: const Text('Save'),
          ),
        ],
      ),
    );
    controller.dispose();
    return result;
  }

  Future<void> _loadAudit() async {
    final entries = await _audit.recent();
    if (mounted) setState(() => _auditEntries = entries);
  }

  void _selectSidebar(_SidebarSection section) {
    FocusScope.of(context).unfocus();
    if (section == _SidebarSection.assistant && _assistant == null) {
      _assistant =
          widget.assistantControllerFactory?.call(_assistantRemoteHosts) ??
          AssistantController(
            remoteHosts: _assistantRemoteHosts,
            appAction: _assistantAppAction,
            captureConnection: _captureConnection,
          );
      unawaited(_startAssistant());
    }
    if (section == _SidebarSection.tools) _toolsOpened = true;
    setState(() => _sidebarSection = section);
    if (section == _SidebarSection.connections) _syncRemotePathField();
    if (section == _SidebarSection.logs) _refreshLogs();
  }

  Future<Map<String, dynamic>> _captureConnection() async {
    final tab = _selectedTab;
    if (tab == null || tab.state != _TabState.connected)
      throw StateError('Select a connected tab first.');
    final desktop = tab.remoteDesktop;
    final image = desktop != null
        ? await AdStreamSurface.capture(desktop.textureId)
        : await (() async {
            await WidgetsBinding.instance.endOfFrame;
            final boundary = _connectionCaptureKey.currentContext
                ?.findRenderObject();
            if (boundary is! RenderRepaintBoundary ||
                boundary.debugNeedsPaint) {
              throw StateError(
                'The connection screen is not ready. Try again.',
              );
            }
            final longest = boundary.size.longestSide;
            return boundary.toImage(
              pixelRatio: longest > 1600 ? 1600 / longest : 1,
            );
          })();
    if (!mounted || _selectedTab != tab || tab.state != _TabState.connected) {
      image.dispose();
      throw StateError(
        'The active connection changed during capture. Try again.',
      );
    }
    return encodeConnectionScreen(
      image,
      '${tab.profile.name} (${tab.profile.connectionType.label})',
    );
  }

  Future<Object> _assistantAppAction(
    String action,
    Map<String, dynamic> args,
    String? remoteSessionId,
  ) async {
    if (!mounted) throw StateError('AetherDesk is closing.');
    if (action == 'app_state') {
      return {
        'activeConnectionId': _selectedTab?.id,
        'connections': [
          for (final tab in _tabs)
            {
              'id': tab.id,
              'name': tab.profile.name,
              'type': tab.profile.connectionType.label,
              'connected': tab.state == _TabState.connected,
            },
        ],
      };
    }
    if (action == 'app_capture') {
      if (args['connectionId'] != _selectedTab?.id)
        throw StateError(
          'Select the requested connection before capturing it.',
        );
      return _captureConnection();
    }
    if (action == 'select_connection') {
      final tab = _tabs
          .where((tab) => tab.id == args['connectionId'])
          .firstOrNull;
      if (tab == null)
        throw StateError('This connection is no longer available.');
      _selectConnectionTab(tab);
      return {'selected': tab.id, 'workspaceChanged': false};
    }
    if (action == 'show_tools' || action == 'show_logs') {
      _selectSidebar(
        action == 'show_tools' ? _SidebarSection.tools : _SidebarSection.logs,
      );
      return {'opened': action};
    }
    if (action != 'open_file' && action != 'open_folder')
      throw StateError('Unsupported AetherDesk action.');
    final remote = remoteSessionId != null;
    final tab = remote
        ? _tabs.where((tab) => tab.sessionId == remoteSessionId).firstOrNull
        : _selectedTab;
    final workspace = tab?.workspaceKey.currentState;
    if (remote && workspace == null)
      throw StateError('The selected SSH file workspace is not available.');
    if (workspace != null) {
      await workspace.assistantOpen(
        args['path'] as String,
        remote: remote,
        folder: action == 'open_folder',
      );
      if (mounted && tab != null) _selectConnectionTab(tab);
    } else if (action == 'open_folder') {
      await Process.start('explorer.exe', [
        args['path'] as String,
      ], mode: ProcessStartMode.detached);
    } else {
      if (_assistantLocalActions == null) {
        final files = FileOpenController(
          identity: 'assistant-local',
          execute: (_, {job, onProgress}) async =>
              throw StateError('Choose an SSH workspace for remote files.'),
        );
        await files.initialize();
        final editor = EditorPopupHost(
          files: files,
          sessionId: 'assistant-local',
          title: 'Local files',
          refresh: () async {},
        );
        _assistantLocalEditor = editor;
        _assistantLocalActions = FileWorkspaceActions(
          context: () => context,
          controller: files,
          editor: editor,
          showEditor: editor.show,
          refresh: () async {},
        );
      }
      await _assistantLocalActions!.open(args['path'] as String, remote: false);
    }
    return {'opened': args['path'], 'remote': remote};
  }

  Future<void> _startAssistant() async {
    try {
      await _assistant!.start();
    } on Object catch (error) {
      _showMessage('Could not start AI Assistant: $error');
    }
  }

  Future<void> _disposeAssistant(AssistantController assistant) async {
    try {
      await _shutdownAssistantSafely(assistant);
    } finally {
      assistant.dispose();
    }
  }

  Future<void> _shutdownAssistantSafely(AssistantController assistant) async {
    try {
      await assistant.shutdown();
    } on Object catch (error) {
      if (_assistantShutdownWarningRecorded) return;
      _assistantShutdownWarningRecorded = true;
      // Closing the app must remain possible after an assistant failure. Log
      // the error type without recording provider credentials or request data.
      try {
        await _audit
            .record(
              event: 'assistant_shutdown',
              result: 'cleanup_warning',
              detail: 'Assistant cleanup reported ${error.runtimeType}.',
            )
            .timeout(const Duration(seconds: 2));
      } on Object {
        debugPrint('AI Assistant cleanup warning could not be recorded.');
      }
    }
  }

  void _showUnavailableCapability(String capability) {
    showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        icon: const Icon(Icons.extension_off_outlined),
        title: Text('$capability backend not installed'),
        content: const SizedBox(
          width: 500,
          child: Text(
            'This build exposes the production interface and capability boundary, but it does not contain a remote-display agent for this mode. AetherDesk will not simulate a successful session. Install and sign a compatible backend before enabling it.',
          ),
        ),
        actions: [
          FilledButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Close'),
          ),
        ],
      ),
    );
  }

  void _showMessage(String message, {NoticeKind kind = NoticeKind.error}) {
    if (!mounted) return;
    AetherNotice.show(context, message: message, kind: kind);
  }

  Future<void> _recordAudit({
    required String event,
    required String result,
    String? profileId,
    String? profileName,
    String? host,
    String? detail,
  }) async {
    try {
      await _audit
          .record(
            event: event,
            result: result,
            profileId: profileId,
            profileName: profileName,
            host: host,
            detail: detail,
          )
          .timeout(const Duration(seconds: 2));
    } on Object catch (error) {
      if (!_auditWarningShown && mounted) {
        _auditWarningShown = true;
        _showMessage('Audit log unavailable: $error');
      }
    }
  }

  @override
  void dispose() {
    unawaited(
      _assistantLocalEditor?.detach().catchError((Object _) {}) ??
          Future<void>.value(),
    );
    _exitListener.dispose();
    _x11HealthTimer?.cancel();
    _hostLogTimer?.cancel();
    final assistant = _assistant;
    if (assistant != null) unawaited(_disposeAssistant(assistant));
    for (final tab in _tabs) {
      final remoteDesktop = tab.remoteDesktop;
      if (remoteDesktop != null) unawaited(remoteDesktop.dispose());
      final sessionId = tab.sessionId;
      if (sessionId != null) unawaited(NativeBridge.disconnect(sessionId));
    }
    unawaited(_x11Server.stop());
    _searchController.dispose();
    _remotePathController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(
          LogicalKeyboardKey.keyK,
          control: true,
          shift: true,
        ): () =>
            _showSessionEditor(),
        const SingleActivator(
          LogicalKeyboardKey.keyB,
          control: true,
          shift: true,
        ): () =>
            setState(() => _sidebarVisible = !_sidebarVisible),
        const SingleActivator(LogicalKeyboardKey.tab, control: true): () =>
            _selectRelativeTab(1),
        const SingleActivator(
          LogicalKeyboardKey.tab,
          control: true,
          shift: true,
        ): () =>
            _selectRelativeTab(-1),
        const SingleActivator(LogicalKeyboardKey.pageDown, control: true): () =>
            _selectRelativeTab(1),
        const SingleActivator(LogicalKeyboardKey.pageUp, control: true): () =>
            _selectRelativeTab(-1),
      },
      child: Focus(
        autofocus: true,
        child: Scaffold(
          body: Column(
            children: [
              _buildTopBar(),
              if (_nativeError != null && _nativeErrorNoticeVisible)
                AutoDismissBanner(
                  key: ValueKey(_nativeError),
                  message: 'Native core could not be loaded: $_nativeError',
                  leading: const Icon(Icons.error_outline_rounded),
                  onDismiss: () =>
                      setState(() => _nativeErrorNoticeVisible = false),
                  actions: [
                    TextButton(
                      onPressed: () {
                        try {
                          final bridge = NativeBridge();
                          bridge.versionSync();
                          setState(() {
                            _capabilities = bridge.capabilitiesSync();
                            _nativeError = null;
                            _nativeErrorNoticeVisible = false;
                          });
                        } on Object catch (error) {
                          setState(() {
                            _nativeError = error.toString();
                            _nativeErrorNoticeVisible = true;
                          });
                        }
                      },
                      child: const Text('Retry'),
                    ),
                  ],
                ),
              Expanded(
                child: LayoutBuilder(
                  builder: (context, constraints) {
                    final maxPanelWidth = (constraints.maxWidth - 360)
                        .clamp(0.0, 900.0)
                        .toDouble();
                    final minPanelWidth = maxPanelWidth < 280
                        ? maxPanelWidth
                        : 280.0;
                    final panelWidth = _sidebarWidth
                        .clamp(minPanelWidth, maxPanelWidth)
                        .toDouble();
                    return Row(
                      children: [
                        Visibility(
                          visible: _sidebarVisible,
                          maintainState: true,
                          child: _buildSidebar(panelWidth),
                        ),
                        if (_sidebarVisible)
                          MouseRegion(
                            cursor: SystemMouseCursors.resizeLeftRight,
                            child: GestureDetector(
                              key: const Key('workspace-panel-resize'),
                              behavior: HitTestBehavior.opaque,
                              dragStartBehavior: DragStartBehavior.down,
                              onHorizontalDragStart: (details) {
                                _sidebarDragStartX = details.globalPosition.dx;
                                _sidebarDragStartWidth = panelWidth;
                              },
                              onHorizontalDragUpdate: (details) => setState(() {
                                _sidebarWidth =
                                    (_sidebarDragStartWidth +
                                            details.globalPosition.dx -
                                            _sidebarDragStartX)
                                        .clamp(minPanelWidth, maxPanelWidth)
                                        .toDouble();
                              }),
                              onDoubleTap: () =>
                                  setState(() => _sidebarWidth = 314),
                              child: const SizedBox(
                                width: 8,
                                child: VerticalDivider(width: 8),
                              ),
                            ),
                          ),
                        Expanded(
                          child: KeyedSubtree(
                            key: const Key('main-workspace'),
                            child: RepaintBoundary(
                              key: _connectionCaptureKey,
                              child: _buildWorkspace(),
                            ),
                          ),
                        ),
                      ],
                    );
                  },
                ),
              ),
              _StatusBar(tab: _selectedTab),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildTopBar() {
    final colors = Theme.of(context).colorScheme;
    return Material(
      color: colors.surface,
      child: SizedBox(
        height: 58,
        child: Row(
          children: [
            IconButton(
              tooltip: 'Toggle workspace panel (Ctrl+Shift+B)',
              onPressed: () =>
                  setState(() => _sidebarVisible = !_sidebarVisible),
              icon: const Icon(Icons.menu_rounded),
            ),
            Image.asset(
              'assets/aetherdesk.png',
              width: 36,
              height: 36,
              fit: BoxFit.contain,
              semanticLabel: 'AetherDesk logo',
            ),
            const SizedBox(width: 10),
            const Text(
              'AetherDesk $appVersion',
              style: TextStyle(fontSize: 17, fontWeight: FontWeight.w800),
            ),
            const SizedBox(width: 8),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
              decoration: BoxDecoration(
                color: colors.primaryContainer,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Text(
                'SECURE WORKSPACE',
                style: TextStyle(
                  color: colors.onPrimaryContainer,
                  fontSize: 9,
                  fontWeight: FontWeight.w800,
                  letterSpacing: .7,
                ),
              ),
            ),
            const Spacer(),
            IconButton(
              tooltip: widget.themeMode == ThemeMode.dark
                  ? 'Use light mode'
                  : 'Use dark mode',
              onPressed: widget.onToggleTheme,
              icon: Icon(
                widget.themeMode == ThemeMode.dark
                    ? Icons.light_mode_outlined
                    : Icons.dark_mode_outlined,
              ),
            ),
            IconButton(
              tooltip: 'Terminal appearance',
              onPressed: () => showDialog<void>(
                context: context,
                builder: (context) =>
                    TerminalSettingsPage(manager: widget.terminalThemeManager),
              ),
              icon: const Icon(Icons.palette_outlined),
            ),
            const SizedBox(width: 4),
            FilledButton.icon(
              onPressed: () => _showSessionEditor(),
              icon: const Icon(Icons.add_rounded),
              label: const Text('New Connection'),
            ),
            const SizedBox(width: 12),
          ],
        ),
      ),
    );
  }

  Widget _buildSidebar(double width) {
    final colors = Theme.of(context).colorScheme;
    return SizedBox(
      key: const Key('workspace-sidebar'),
      width: width,
      child: Row(
        children: [
          Material(
            color: colors.surfaceContainerLowest,
            child: SizedBox(
              width: 55,
              child: ListView(
                padding: const EdgeInsets.symmetric(vertical: 8),
                children: _SidebarSection.values
                    .map(
                      (section) => Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 7,
                          vertical: 2,
                        ),
                        child: IconButton(
                          tooltip: section.label,
                          isSelected: _sidebarSection == section,
                          style: IconButton.styleFrom(
                            foregroundColor: _sidebarSection == section
                                ? colors.onPrimaryContainer
                                : colors.onSurfaceVariant,
                            backgroundColor: _sidebarSection == section
                                ? colors.primaryContainer
                                : Colors.transparent,
                          ),
                          onPressed: () => _selectSidebar(section),
                          icon: Icon(section.icon, size: 20),
                        ),
                      ),
                    )
                    .toList(),
              ),
            ),
          ),
          const VerticalDivider(width: 1),
          Expanded(
            child: IndexedStack(
              index: _sidebarSection == _SidebarSection.assistant
                  ? 1
                  : _sidebarSection == _SidebarSection.tools
                  ? 2
                  : 0,
              children: [
                _buildSidebarContent(),
                if (_assistant != null)
                  AssistantPage(controller: _assistant!)
                else
                  const SizedBox.shrink(),
                if (_toolsOpened)
                  ToolsPanel(
                    key: _toolsPanelKey,
                    registry: ToolRegistry.shared,
                    remoteHosts: _assistantRemoteHosts,
                  )
                else
                  const SizedBox.shrink(),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildSidebarContent() {
    return switch (_sidebarSection) {
      _SidebarSection.sessions => _buildSavedSessionsPanel(),
      _SidebarSection.connections => _buildActiveConnectionsPanel(),
      _SidebarSection.logs => _buildLogsPanel(),
      _SidebarSection.bookmarks => const _SidebarInformation(
        title: 'Bookmarks',
        icon: Icons.bookmark_outline_rounded,
        message:
            'Command and path bookmarks are reserved in the workspace architecture.',
      ),
      _SidebarSection.macros => const _SidebarInformation(
        title: 'Macros',
        icon: Icons.bolt_outlined,
        message:
            'Signed automation and macro execution require the policy engine milestone.',
      ),
      _SidebarSection.tools => const SizedBox.shrink(),
      _SidebarSection.monitoring => const _SidebarInformation(
        title: 'Monitoring',
        icon: Icons.monitor_heart_outlined,
        message:
            'Live session health and resource telemetry require the remote agent.',
      ),
      _SidebarSection.assistant => const SizedBox.shrink(),
    };
  }

  Widget _buildSavedSessionsPanel() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 14, 12, 8),
          child: TextField(
            controller: _searchController,
            onChanged: (value) => setState(() => _search = value),
            decoration: const InputDecoration(
              hintText: 'Search connections',
              prefixIcon: Icon(Icons.search_rounded, size: 20),
            ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 7, 8, 7),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  'SAVED SESSIONS',
                  style: TextStyle(
                    color: Theme.of(context).colorScheme.onSurfaceVariant,
                    fontSize: 10,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 1.1,
                  ),
                ),
              ),
              IconButton(
                tooltip: 'New connection',
                onPressed: () => _showSessionEditor(),
                icon: const Icon(Icons.add_rounded, size: 18),
              ),
            ],
          ),
        ),
        Expanded(
          child: _loading
              ? const Center(child: CircularProgressIndicator())
              : _visibleSessions.isEmpty
              ? const Center(
                  child: Padding(
                    padding: EdgeInsets.all(20),
                    child: Text(
                      'No saved connections\nPress Ctrl+Shift+K to create one',
                      textAlign: TextAlign.center,
                    ),
                  ),
                )
              : ListView(
                  children: _visibleSessions.map(_buildSessionRow).toList(),
                ),
        ),
      ],
    );
  }

  Widget _buildActiveConnectionsPanel() {
    final active = _activeConnections;
    final selected = _selectedTab;
    final canChangeDirectory =
        selected?.state == _TabState.connected && _hasRemoteShell(selected);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 14, 12, 8),
          child: TextField(
            key: const ValueKey('active-connection-remote-directory'),
            controller: _remotePathController,
            enabled: canChangeDirectory,
            onSubmitted: _changeRemoteDirectory,
            decoration: InputDecoration(
              labelText: 'Current server directory',
              hintText: canChangeDirectory ? '/home/user' : 'No active shell',
              prefixIcon: const Icon(Icons.folder_open_outlined, size: 20),
              suffixIcon: IconButton(
                tooltip: 'Copy remote directory',
                onPressed: canChangeDirectory ? _copyRemoteDirectory : null,
                icon: const Icon(Icons.copy_rounded, size: 18),
              ),
            ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
          child: Text(
            'ACTIVE CONNECTIONS',
            style: TextStyle(
              color: Theme.of(context).colorScheme.onSurfaceVariant,
              fontSize: 10,
              fontWeight: FontWeight.w800,
              letterSpacing: 1.1,
            ),
          ),
        ),
        Expanded(
          child: active.isEmpty
              ? const Center(
                  child: Padding(
                    padding: EdgeInsets.all(20),
                    child: Text(
                      'No active connections\nOpen a saved session to connect',
                      textAlign: TextAlign.center,
                    ),
                  ),
                )
              : ListView(
                  children: active.map(_buildActiveConnectionRow).toList(),
                ),
        ),
      ],
    );
  }

  Widget _buildActiveConnectionRow(_ConnectionTab tab) {
    final selected = tab.id == _selectedTabId;
    return ListTile(
      selected: selected,
      leading: _TabStateIcon(state: tab.state),
      title: Text(
        tab.profile.name,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      subtitle: Text(
        '${tab.profile.username}@${tab.profile.host}:${tab.profile.port} · '
        '${tab.profile.connectionType.tabSuffix}',
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      onTap: () => _selectConnectionTab(tab),
    );
  }

  static const List<String> _logTabLabels = [
    'Application',
    'Host',
    'Audit',
    'Problems',
  ];

  static const List<String> _logSources = [
    AppLogSource.coreRust,
    AppLogSource.coreFfi,
    AppLogSource.uiDart,
    AppLogSource.uiNative,
    AppLogSource.editor,
    AppLogSource.assistant,
    AppLogSource.x11,
    AppLogSource.hostAgent,
  ];

  void _refreshLogs() {
    if (_hostDesktopId == null) {
      final sessions = _hostLogSessions();
      if (sessions.isNotEmpty) _hostDesktopId = sessions.first.desktopSessionId;
    }
    switch (_logsTab) {
      case 0:
        unawaited(_loadAppLog());
      case 1:
        unawaited(_loadHostLog());
      case 2:
        unawaited(_loadAudit());
      default:
        unawaited(_loadProblems());
    }
    _scheduleHostLogRefresh();
  }

  List<_ConnectionTab> _hostLogSessions() =>
      _tabs.where((tab) => tab.desktopSessionId != null).toList();

  Future<void> _loadAppLog() async {
    final records = await AppLog.instance.recent();
    if (mounted) setState(() => _appLogRecords = records);
  }

  Future<void> _loadProblems() async {
    await _loadAppLog();
    await _loadHostLog(quiet: true);
  }

  Future<void> _loadHostLog({bool quiet = false}) async {
    if (_hostLogFetching) return;
    final desktopId = _hostDesktopId;
    String? sshSessionId;
    for (final tab in _hostLogSessions()) {
      if (tab.desktopSessionId == desktopId && tab.sessionId != null) {
        sshSessionId = tab.sessionId;
        break;
      }
    }
    if (desktopId == null || sshSessionId == null) {
      if (!quiet && mounted) {
        setState(() {
          _hostLogRecords = const [];
          _hostLogNote = 'Connect a remote desktop session to fetch its log.';
          _hostLogBusy = false;
        });
      }
      return;
    }
    if (!quiet && mounted) {
      setState(() {
        _hostLogBusy = true;
        _hostLogNote = null;
      });
    }
    if (!RegExp(
      r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$',
    ).hasMatch(desktopId)) {
      if (mounted)
        setState(() {
          _hostLogNote = 'Invalid desktop session identifier.';
          _hostLogBusy = false;
        });
      return;
    }
    _hostLogFetching = true;
    try {
      // Fixed command, bounded output; the id is generated hex (§3.3).
      final output = await NativeBridge.executeCommand(
        sshSessionId,
        'tail -c 262144 "\$HOME/.aetherdesk/sessions/$desktopId/agent.log" '
        '2>/dev/null || true',
      );
      final records = output
          .split('\n')
          .map(
            (line) => AppLogRecord.tryParse(
              line,
              defaultSource: AppLogSource.hostAgent,
            ),
          )
          .whereType<AppLogRecord>()
          .toList()
          .reversed
          .toList();
      if (!mounted || _hostDesktopId != desktopId) return;
      setState(() {
        _hostLogRecords = records;
        _hostLogBusy = false;
        _hostLogNote = records.isEmpty ? 'The host log is empty.' : null;
      });
    } on Object catch (error) {
      if (!mounted || _hostDesktopId != desktopId) return;
      setState(() {
        _hostLogBusy = false;
        _hostLogNote = 'Host log unavailable: $error';
      });
    } finally {
      _hostLogFetching = false;
      if (mounted && _hostLogBusy) setState(() => _hostLogBusy = false);
    }
  }

  void _scheduleHostLogRefresh() {
    _hostLogTimer?.cancel();
    if (_sidebarSection != _SidebarSection.logs || !_logFollow) return;
    // Follow keeps the open tab live; the Host tab polls over SSH.
    _hostLogTimer = Timer.periodic(const Duration(seconds: 8), (_) {
      if (_sidebarSection != _SidebarSection.logs || !_logFollow) return;
      switch (_logsTab) {
        case 0:
          unawaited(_loadAppLog());
        case 1:
          unawaited(_loadHostLog(quiet: true));
        case 2:
          unawaited(_loadAudit());
        default:
          unawaited(_loadProblems());
      }
    });
  }

  List<AppLogRecord> _filteredRecords(Iterable<AppLogRecord> records) {
    final query = _logCaseSensitive ? _logQuery : _logQuery.toLowerCase();
    return records.where((record) {
      if (!_logLevelFilters.contains(record.level)) return false;
      if (_logSourceFilter != null && record.source != _logSourceFilter) {
        return false;
      }
      if (query.isEmpty) return true;
      final haystack = _logCaseSensitive
          ? '${record.message}\n${record.error ?? ''}\n${record.target ?? ''}'
          : '${record.message}\n${record.error ?? ''}\n${record.target ?? ''}'
                .toLowerCase();
      return haystack.contains(query);
    }).toList();
  }

  List<AppLogRecord> _visibleLogRecords() => switch (_logsTab) {
    1 => _filteredRecords(_hostLogRecords),
    2 => _filteredRecords(
      _auditEntries.map(
        (entry) => AppLogRecord(
          timestamp: entry.timestamp,
          level: entry.result == 'failure'
              ? AppLogLevel.error
              : AppLogLevel.info,
          source: 'audit',
          message: '${entry.event}: ${entry.result}',
          profile: entry.profileName,
          host: entry.host,
          error: entry.detail,
        ),
      ),
    ),
    3 => _filteredRecords([
      ..._appLogRecords.where(
        (record) => record.level.rank >= AppLogLevel.warn.rank,
      ),
      ..._hostLogRecords.where(
        (record) => record.level.rank >= AppLogLevel.warn.rank,
      ),
    ]),
    _ => _filteredRecords(_appLogRecords),
  };

  String get _logsPathLabel => switch (_logsTab) {
    1 => 'Host · ~/.aetherdesk/sessions/<session>/agent.log',
    2 => _audit.path,
    3 => 'Derived from Application + Host',
    _ => AppLog.instance.path,
  };

  Future<void> _copyVisibleLogs() async {
    final buffer = StringBuffer();
    for (final record in _visibleLogRecords().reversed) {
      buffer.writeln(jsonEncode(record.toJson()));
    }
    await Clipboard.setData(ClipboardData(text: buffer.toString()));
    if (!mounted) return;
    AetherNotice.show(context, message: 'Visible log records copied');
  }

  Future<void> _exportVisibleLogs() async {
    final records = _visibleLogRecords();
    final location = await getSaveLocation(
      suggestedName: 'aetherdesk-logs.jsonl',
    );
    if (location == null) return;
    final buffer = StringBuffer();
    for (final record in records.reversed) {
      buffer.writeln(jsonEncode(record.toJson()));
    }
    await File(location.path).writeAsString(buffer.toString());
    if (!mounted) return;
    AetherNotice.show(
      context,
      message: 'Exported ${records.length} log records to ${location.path}',
    );
  }

  Future<void> _openLogFolder() async {
    final directory = AppLog.instance.directory;
    await directory.create(recursive: true);
    if (Platform.isWindows) {
      await Process.start('explorer', [
        directory.path,
      ], mode: ProcessStartMode.detached);
    } else {
      await Process.start('xdg-open', [
        directory.path,
      ], mode: ProcessStartMode.detached);
    }
  }

  Widget _buildLogRecordTile(AppLogRecord record) {
    final scheme = Theme.of(context).colorScheme;
    final (badge, color) = switch (record.level) {
      AppLogLevel.debug => ('DBG', scheme.onSurfaceVariant),
      AppLogLevel.info => ('INF', scheme.primary),
      AppLogLevel.warn => ('WRN', Colors.orange),
      AppLogLevel.error => ('ERR', scheme.error),
    };
    return ExpansionTile(
      tilePadding: const EdgeInsets.symmetric(horizontal: 12),
      leading: Container(
        padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 2),
        decoration: BoxDecoration(
          border: Border.all(color: color),
          borderRadius: BorderRadius.circular(4),
        ),
        child: Text(
          badge,
          style: TextStyle(
            fontSize: 9,
            fontWeight: FontWeight.w700,
            color: color,
          ),
        ),
      ),
      title: Text(
        record.message,
        maxLines: 2,
        overflow: TextOverflow.ellipsis,
        style: const TextStyle(fontSize: 12),
      ),
      subtitle: Text(
        '${record.timestamp.toLocal()} · ${record.source}'
        '${record.target == null ? '' : ' · ${record.target}'}',
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(fontSize: 10, color: scheme.onSurfaceVariant),
      ),
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
          child: SelectableText(
            record.error == null
                ? record.message
                : '${record.message}\n\n${record.error}',
            style: const TextStyle(fontSize: 11),
          ),
        ),
      ],
    );
  }

  Widget _buildLogsPanel() {
    final scheme = Theme.of(context).colorScheme;
    final records = _visibleLogRecords();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 8, 10),
          child: Row(
            children: [
              const Expanded(
                child: Text(
                  'LOGS',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 1.1,
                  ),
                ),
              ),
              if (_hostLogBusy)
                const Padding(
                  padding: EdgeInsets.only(right: 8),
                  child: SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  ),
                ),
              IconButton(
                tooltip: 'Refresh',
                onPressed: _refreshLogs,
                icon: const Icon(Icons.refresh_rounded, size: 19),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: SegmentedButton<int>(
            showSelectedIcon: false,
            style: const ButtonStyle(
              visualDensity: VisualDensity.compact,
              tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            segments: [
              for (var index = 0; index < _logTabLabels.length; index++)
                ButtonSegment(
                  value: index,
                  label: Text(
                    _logTabLabels[index],
                    style: const TextStyle(fontSize: 11),
                  ),
                ),
            ],
            selected: {_logsTab},
            onSelectionChanged: (selection) {
              setState(() => _logsTab = selection.first);
              _refreshLogs();
            },
          ),
        ),
        if (_logsTab == 1)
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 6, 12, 0),
            child: DropdownButton<String>(
              value:
                  _hostLogSessions().any(
                    (tab) => tab.desktopSessionId == _hostDesktopId,
                  )
                  ? _hostDesktopId
                  : null,
              isDense: true,
              isExpanded: true,
              hint: const Text(
                'Remote desktop session',
                style: TextStyle(fontSize: 11),
              ),
              style: const TextStyle(fontSize: 11),
              items: [
                for (final tab in _hostLogSessions())
                  DropdownMenuItem(
                    value: tab.desktopSessionId,
                    child: Text(
                      '${tab.profile.name} · ${tab.profile.host}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
              ],
              onChanged: (value) {
                setState(() => _hostDesktopId = value);
                unawaited(_loadHostLog());
              },
            ),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 6, 12, 0),
          child: Wrap(
            spacing: 6,
            runSpacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              for (final level in AppLogLevel.values)
                FilterChip(
                  label: Text(
                    level.label,
                    style: const TextStyle(fontSize: 10),
                  ),
                  visualDensity: VisualDensity.compact,
                  selected: _logLevelFilters.contains(level),
                  onSelected: (selected) {
                    setState(() {
                      if (selected) {
                        _logLevelFilters.add(level);
                      } else if (_logLevelFilters.length > 1) {
                        _logLevelFilters.remove(level);
                      }
                    });
                  },
                ),
              DropdownButton<String?>(
                value: _logSourceFilter,
                isDense: true,
                hint: const Text('source', style: TextStyle(fontSize: 11)),
                style: const TextStyle(fontSize: 11),
                items: [
                  const DropdownMenuItem(
                    value: null,
                    child: Text('all sources'),
                  ),
                  for (final source in _logSources)
                    DropdownMenuItem(value: source, child: Text(source)),
                ],
                onChanged: (value) => setState(() => _logSourceFilter = value),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 6, 12, 0),
          child: Row(
            children: [
              Expanded(
                child: TextField(
                  style: const TextStyle(fontSize: 12),
                  decoration: const InputDecoration(
                    isDense: true,
                    hintText: 'Search messages…',
                    prefixIcon: Icon(Icons.search, size: 16),
                    border: OutlineInputBorder(),
                  ),
                  onChanged: (value) => setState(() => _logQuery = value),
                ),
              ),
              IconButton(
                tooltip: 'Match case',
                onPressed: () =>
                    setState(() => _logCaseSensitive = !_logCaseSensitive),
                icon: Icon(
                  Icons.text_fields,
                  size: 18,
                  color: _logCaseSensitive
                      ? scheme.primary
                      : scheme.onSurfaceVariant,
                ),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(14, 8, 14, 0),
          child: Text(
            _logsPathLabel,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: 10,
              color: Theme.of(context).colorScheme.onSurfaceVariant,
            ),
          ),
        ),
        const SizedBox(height: 8),
        const Divider(height: 1),
        Expanded(
          child: records.isEmpty
              ? Center(
                  child: Text(
                    _logsTab == 1
                        ? (_hostLogNote ??
                              'Host log unavailable — connect the session')
                        : 'No log records yet',
                    textAlign: TextAlign.center,
                  ),
                )
              : ListView.builder(
                  itemCount: records.length,
                  itemBuilder: (context, index) =>
                      _buildLogRecordTile(records[index]),
                ),
        ),
        const Divider(height: 1),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8),
          child: Row(
            children: [
              const Text('Follow', style: TextStyle(fontSize: 11)),
              Switch(
                value: _logFollow,
                onChanged: (value) {
                  setState(() => _logFollow = value);
                  _scheduleHostLogRefresh();
                },
              ),
              const Spacer(),
              IconButton(
                tooltip: 'Copy visible records',
                onPressed: _copyVisibleLogs,
                icon: const Icon(Icons.copy_all_outlined, size: 18),
              ),
              IconButton(
                tooltip: 'Export visible records…',
                onPressed: _exportVisibleLogs,
                icon: const Icon(Icons.save_alt_outlined, size: 18),
              ),
              IconButton(
                tooltip: 'Open log folder',
                onPressed: _openLogFolder,
                icon: const Icon(Icons.folder_open_outlined, size: 18),
              ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildSessionRow(SavedSession session) {
    final selected = session.id == _selectedTab?.profile.id;
    return GestureDetector(
      onSecondaryTapDown: (details) async {
        final action = await showMenu<String>(
          context: context,
          position: RelativeRect.fromLTRB(
            details.globalPosition.dx,
            details.globalPosition.dy,
            details.globalPosition.dx,
            details.globalPosition.dy,
          ),
          items: [
            const PopupMenuItem(
              value: 'open',
              child: _MenuLabel(
                Icons.add_to_photos_outlined,
                'Open in new tab',
              ),
            ),
            const PopupMenuDivider(),
            const PopupMenuItem(
              value: 'edit',
              child: _MenuLabel(Icons.edit_outlined, 'Edit'),
            ),
            const PopupMenuItem(
              value: 'rename',
              child: _MenuLabel(Icons.drive_file_rename_outline, 'Rename'),
            ),
            const PopupMenuItem(
              value: 'duplicate',
              child: _MenuLabel(Icons.copy_rounded, 'Duplicate'),
            ),
            PopupMenuItem(
              value: 'favorite',
              child: _MenuLabel(
                session.favorite
                    ? Icons.star_outline_rounded
                    : Icons.star_rounded,
                session.favorite ? 'Remove favorite' : 'Add to favorites',
              ),
            ),
            if (session.rememberPassword)
              const PopupMenuItem(
                value: 'forgetCredential',
                child: _MenuLabel(
                  Icons.key_off_outlined,
                  'Forget stored credential',
                ),
              ),
            const PopupMenuDivider(),
            const PopupMenuItem(
              value: 'delete',
              child: _MenuLabel(Icons.delete_outline_rounded, 'Delete'),
            ),
          ],
        );
        if (action != null) await _sessionAction(action, session);
      },
      child: ListTile(
        selected: selected,
        dense: true,
        leading: Icon(
          _connectionIcon(session.connectionType),
          color: selected
              ? Theme.of(context).colorScheme.primary
              : Theme.of(context).colorScheme.onSurfaceVariant,
        ),
        title: Row(
          children: [
            Expanded(
              child: Text(
                session.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            if (session.rememberPassword)
              const Padding(
                padding: EdgeInsets.only(right: 5),
                child: Icon(Icons.lock_outline_rounded, size: 13),
              ),
            if (session.favorite)
              const Icon(
                Icons.star_rounded,
                color: Color(0xFFF0C66C),
                size: 15,
              ),
          ],
        ),
        subtitle: Text(
          '${session.username.isEmpty ? 'username required' : session.username}@${session.host}:${session.port} · ${session.connectionType.tabSuffix}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
        ),
        onTap: () => _openSaved(session),
      ),
    );
  }

  Widget _buildWorkspace() {
    if (_tabs.isEmpty) {
      return _WorkspaceLauncher(
        onConnect: () => _showSessionEditor(),
        capabilities: _capabilities,
      );
    }
    final selectedIndex = _tabs.indexWhere((tab) => tab.id == _selectedTabId);
    return Column(
      children: [
        _buildTabStrip(),
        Expanded(
          child: IndexedStack(
            index: selectedIndex < 0 ? 0 : selectedIndex,
            children: _tabs.map(_buildTabWorkspace).toList(),
          ),
        ),
      ],
    );
  }

  Widget _buildTabStrip() {
    final colors = Theme.of(context).colorScheme;
    return Material(
      color: colors.surfaceContainerLow,
      child: SessionTabStrip<_ConnectionTab>(
        tabs: _tabs,
        selectedId: _selectedTabId,
        idOf: (tab) => tab.id,
        onSelected: _selectConnectionTab,
        onReorder: (oldIndex, newIndex) {
          setState(() {
            final item = _tabs.removeAt(oldIndex);
            _tabs.insert(newIndex, item);
          });
        },
        itemBuilder: (context, tab, selected) => Container(
          padding: const EdgeInsets.only(left: 12),
          decoration: BoxDecoration(
            color: selected ? colors.surface : Colors.transparent,
            border: Border(
              bottom: BorderSide(
                color: selected ? colors.primary : Colors.transparent,
                width: 2,
              ),
              right: BorderSide(color: colors.outlineVariant),
            ),
          ),
          child: Row(
            children: [
              _TabStateIcon(state: tab.state),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  '${tab.profile.name} - ${tab.profile.connectionType.tabSuffix}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              const SizedBox(width: 6),
              IconButton(
                tooltip: 'Close tab',
                onPressed: () => _closeTab(tab),
                icon: const Icon(Icons.close_rounded, size: 16),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildTabWorkspace(_ConnectionTab tab) {
    switch (tab.state) {
      case _TabState.connecting:
        return _ConnectionStateView(
          icon: Icons.lock_clock_outlined,
          title: 'Establishing encrypted session',
          message:
              '${tab.profile.username}@${tab.profile.host}:${tab.profile.port}',
          busy: true,
        );
      case _TabState.connected:
        if (tab.profile.connectionType == ConnectionType.remoteDesktop) {
          final desktop = tab.remoteDesktop;
          if (desktop != null)
            return RemoteDesktopView(key: ValueKey(tab.id), session: desktop);
          return const Center(child: CircularProgressIndicator());
        }
        return ConnectedWorkspace(
          key: tab.workspaceKey,
          sessionId: tab.sessionId!,
          profile: tab.profile,
          terminalThemeManager: widget.terminalThemeManager,
          onDisconnect: () => _disconnectTab(tab),
          onConnectionLost: (error) => unawaited(
            _handleConnectionLost(tab, 'The SSH connection was lost: $error'),
          ),
          onRemoteDirectoryChanged: (path) =>
              _handleRemoteDirectoryChanged(tab, path),
          onAudit: (event, result, detail) => unawaited(
            _recordAudit(
              event: event,
              result: result,
              profileId: tab.profile.id,
              profileName: tab.profile.name,
              host: tab.profile.host,
              detail: detail,
            ),
          ),
        );
      case _TabState.disconnected:
        return _ConnectionStateView(
          icon: Icons.link_off_rounded,
          title: 'Session disconnected',
          message: tab.error ?? 'The tab and workspace state are preserved.',
          actionLabel: 'Reconnect',
          onAction: () => _reconnectTab(tab),
        );
      case _TabState.failed:
        return _ConnectionStateView(
          icon: Icons.error_outline_rounded,
          title: 'Connection failed',
          message: tab.error ?? 'The server did not accept the connection.',
          actionLabel: 'Try again',
          onAction: () => _reconnectTab(tab),
        );
    }
  }
}

class _WorkspaceLauncher extends StatelessWidget {
  const _WorkspaceLauncher({
    required this.onConnect,
    required this.capabilities,
  });

  final VoidCallback onConnect;
  final CoreCapabilities? capabilities;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final features = [
      _WelcomeFeature(
        icon: Icons.terminal_rounded,
        title: 'SSH Terminal',
        description: 'Fast interactive shells with verified host keys.',
        status: capabilities?.ssh == true ? 'Ready' : 'Unavailable',
        ready: capabilities?.ssh == true,
      ),
      _WelcomeFeature(
        icon: Icons.folder_copy_outlined,
        title: 'SFTP Workspace',
        description: 'Local and remote files beside every live session.',
        status: capabilities?.sftp == true ? 'Ready' : 'Unavailable',
        ready: capabilities?.sftp == true,
      ),
      _WelcomeFeature(
        icon: Icons.desktop_windows_outlined,
        title: 'Remote Desktop',
        description: capabilities?.remoteDesktop == true
            ? 'Embedded FreeRDP desktop rendered inside AetherDesk.'
            : 'Embedded FreeRDP engine is not linked in this build.',
        status: capabilities?.remoteDesktop == true ? 'Ready' : 'Unavailable',
        ready: capabilities?.remoteDesktop == true,
      ),
      _WelcomeFeature(
        icon: Icons.open_in_new_rounded,
        title: 'SSH + X11',
        description: 'Launch individual Linux GUI applications securely.',
        status: capabilities?.sshX11 == true ? 'Ready' : 'Unavailable',
        ready: capabilities?.sshX11 == true,
      ),
    ];
    return ColoredBox(
      color: colors.surfaceContainerLowest,
      child: SingleChildScrollView(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 920),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 40, vertical: 56),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Container(
                        width: 44,
                        height: 44,
                        decoration: BoxDecoration(
                          color: colors.primaryContainer,
                          borderRadius: BorderRadius.circular(14),
                        ),
                        child: Icon(
                          Icons.shield_outlined,
                          color: colors.onPrimaryContainer,
                        ),
                      ),
                      const SizedBox(width: 14),
                      Text(
                        'SECURE REMOTE WORKSPACE',
                        style: TextStyle(
                          color: colors.primary,
                          fontSize: 12,
                          fontWeight: FontWeight.w800,
                          letterSpacing: 1.15,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 24),
                  Text(
                    'Everything remote.\nOne focused workspace.',
                    style: TextStyle(
                      fontSize: 38,
                      height: 1.08,
                      fontWeight: FontWeight.w800,
                      letterSpacing: -1.1,
                      color: colors.onSurface,
                    ),
                  ),
                  const SizedBox(height: 16),
                  Text(
                    'Open every connection in its own independent tab, keep credentials protected by Windows, and move between terminal, files, applications, and desktops without losing context.',
                    style: TextStyle(
                      fontSize: 15,
                      height: 1.55,
                      color: colors.onSurfaceVariant,
                    ),
                  ),
                  const SizedBox(height: 28),
                  Row(
                    children: [
                      FilledButton.icon(
                        onPressed: onConnect,
                        icon: const Icon(Icons.add_link_rounded),
                        label: const Text('New Connection'),
                      ),
                      const SizedBox(width: 14),
                      Text(
                        'Ctrl + Shift + K',
                        style: TextStyle(
                          color: colors.onSurfaceVariant,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 36),
                  LayoutBuilder(
                    builder: (context, constraints) {
                      final cardWidth = constraints.maxWidth >= 720
                          ? (constraints.maxWidth - 14) / 2
                          : constraints.maxWidth;
                      return Wrap(
                        spacing: 14,
                        runSpacing: 14,
                        children: features
                            .map(
                              (feature) => SizedBox(
                                width: cardWidth,
                                child: _WelcomeFeatureCard(feature: feature),
                              ),
                            )
                            .toList(),
                      );
                    },
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

class _WelcomeFeature {
  const _WelcomeFeature({
    required this.icon,
    required this.title,
    required this.description,
    required this.status,
    required this.ready,
  });

  final IconData icon;
  final String title;
  final String description;
  final String status;
  final bool ready;
}

class _WelcomeFeatureCard extends StatelessWidget {
  const _WelcomeFeatureCard({required this.feature});

  final _WelcomeFeature feature;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final statusColor = feature.ready ? const Color(0xFF43B878) : colors.error;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: colors.surfaceContainerLow,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: colors.outlineVariant),
      ),
      child: Padding(
        padding: const EdgeInsets.all(18),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Container(
              width: 38,
              height: 38,
              decoration: BoxDecoration(
                color: colors.secondaryContainer,
                borderRadius: BorderRadius.circular(11),
              ),
              child: Icon(
                feature.icon,
                size: 20,
                color: colors.onSecondaryContainer,
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          feature.title,
                          style: const TextStyle(fontWeight: FontWeight.w700),
                        ),
                      ),
                      Icon(Icons.circle, size: 8, color: statusColor),
                      const SizedBox(width: 6),
                      Text(
                        feature.status,
                        style: TextStyle(
                          color: statusColor,
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 7),
                  Text(
                    feature.description,
                    style: TextStyle(
                      color: colors.onSurfaceVariant,
                      height: 1.35,
                      fontSize: 12.5,
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ConnectionStateView extends StatelessWidget {
  const _ConnectionStateView({
    required this.icon,
    required this.title,
    required this.message,
    this.busy = false,
    this.actionLabel,
    this.onAction,
  });

  final IconData icon;
  final String title;
  final String message;
  final bool busy;
  final String? actionLabel;
  final VoidCallback? onAction;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 620),
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 48),
              const SizedBox(height: 18),
              Text(
                title,
                style: const TextStyle(
                  fontSize: 22,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 10),
              SelectableText(message, textAlign: TextAlign.center),
              if (busy) ...[
                const SizedBox(height: 22),
                const SizedBox(width: 220, child: LinearProgressIndicator()),
              ],
              if (actionLabel != null) ...[
                const SizedBox(height: 22),
                FilledButton.icon(
                  onPressed: onAction,
                  icon: const Icon(Icons.refresh_rounded),
                  label: Text(actionLabel!),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _SidebarInformation extends StatelessWidget {
  const _SidebarInformation({
    required this.title,
    required this.icon,
    required this.message,
  });

  final String title;
  final IconData icon;
  final String message;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 28),
          const SizedBox(height: 12),
          Text(
            title,
            style: const TextStyle(fontSize: 18, fontWeight: FontWeight.w700),
          ),
          const SizedBox(height: 10),
          Text(
            message,
            style: TextStyle(
              height: 1.45,
              color: Theme.of(context).colorScheme.onSurfaceVariant,
            ),
          ),
        ],
      ),
    );
  }
}

class _StatusBar extends StatefulWidget {
  const _StatusBar({required this.tab});
  final _ConnectionTab? tab;

  @override
  State<_StatusBar> createState() => _StatusBarState();
}

class _StatusBarState extends State<_StatusBar> {
  late Timer _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _timer.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final now = DateTime.now();
    final clock =
        '${now.hour.toString().padLeft(2, '0')}:${now.minute.toString().padLeft(2, '0')}:${now.second.toString().padLeft(2, '0')}';
    final tab = widget.tab;
    final colors = Theme.of(context).colorScheme;
    final connected = tab?.state == _TabState.connected;
    return Container(
      height: 29,
      color: colors.surface,
      padding: const EdgeInsets.symmetric(horizontal: 12),
      child: Row(
        children: [
          Icon(
            connected ? Icons.circle : Icons.circle_outlined,
            size: 9,
            color: connected
                ? const Color(0xFF45B87B)
                : tab?.state == _TabState.connecting
                ? Colors.amber
                : colors.onSurfaceVariant,
          ),
          const SizedBox(width: 8),
          Text(
            tab == null
                ? 'No active session'
                : '${tab.state.name} · ${tab.profile.host}',
            style: TextStyle(fontSize: 12, color: colors.onSurfaceVariant),
          ),
          const Spacer(),
          if (tab != null)
            Text(
              '${tab.profile.connectionType.label} · ${tab.profile.authentication.label}',
              style: TextStyle(fontSize: 11, color: colors.onSurfaceVariant),
            ),
          const SizedBox(width: 18),
          Text(
            clock,
            style: TextStyle(
              fontFamily: 'monospace',
              fontSize: 12,
              color: colors.onSurfaceVariant,
            ),
          ),
        ],
      ),
    );
  }
}

class _TabStateIcon extends StatelessWidget {
  const _TabStateIcon({required this.state});
  final _TabState state;

  @override
  Widget build(BuildContext context) {
    if (state == _TabState.connecting) {
      return const SizedBox(
        width: 14,
        height: 14,
        child: CircularProgressIndicator(strokeWidth: 2),
      );
    }
    return Icon(
      switch (state) {
        _TabState.connected => Icons.circle,
        _TabState.disconnected => Icons.circle_outlined,
        _TabState.failed => Icons.error_outline_rounded,
        _TabState.connecting => Icons.circle,
      },
      size: 13,
      color: switch (state) {
        _TabState.connected => const Color(0xFF45B87B),
        _TabState.failed => Theme.of(context).colorScheme.error,
        _ => Theme.of(context).colorScheme.onSurfaceVariant,
      },
    );
  }
}

class _SessionRequest {
  const _SessionRequest({
    required this.profile,
    required this.secret,
    required this.masterPassword,
    required this.save,
    required this.connect,
  });

  final SavedSession profile;
  final String secret;
  final String? masterPassword;
  final bool save;
  final bool connect;
}

class _SessionDialog extends StatefulWidget {
  const _SessionDialog({
    required this.existing,
    required this.connectByDefault,
    required this.capabilities,
    required this.storedCredential,
  });

  final SavedSession? existing;
  final bool connectByDefault;
  final CoreCapabilities? capabilities;
  final CredentialMetadata? storedCredential;

  @override
  State<_SessionDialog> createState() => _SessionDialogState();
}

class _SessionDialogState extends State<_SessionDialog> {
  final _formKey = GlobalKey<FormState>();
  late final TextEditingController _name;
  late final TextEditingController _host;
  late final TextEditingController _port;
  late final TextEditingController _username;
  late final TextEditingController _secret;
  late final TextEditingController _keyPath;
  String? _wireguardConfigPath;
  late final TextEditingController _masterPassword;
  late final TextEditingController _masterPasswordConfirmation;
  late ConnectionType _connectionType;
  late AuthenticationMethod _authentication;
  late DesktopEnvironment _desktopEnvironment;
  bool _save = true;
  bool _rememberUsername = true;
  bool _rememberPassword = false;
  bool _masterPasswordProtected = false;
  bool _hideSecret = true;
  bool _hideMaster = true;
  String? _formMessage;

  @override
  void initState() {
    super.initState();
    final existing = widget.existing;
    _name = TextEditingController(text: existing?.name ?? 'New connection');
    _host = TextEditingController(text: existing?.host ?? '');
    _port = TextEditingController(text: (existing?.port ?? 22).toString());
    _username = TextEditingController(text: existing?.username ?? '');
    _secret = TextEditingController();
    _keyPath = TextEditingController(text: existing?.keyPath ?? '');
    _wireguardConfigPath = existing?.wireguardConfigPath;
    _masterPassword = TextEditingController();
    _masterPasswordConfirmation = TextEditingController();
    _connectionType = existing?.connectionType ?? ConnectionType.sshTerminal;
    _authentication = existing?.authentication ?? AuthenticationMethod.password;
    _desktopEnvironment =
        existing?.desktopEnvironment ?? DesktopEnvironment.xfce;
    _rememberUsername = existing?.rememberUsername ?? true;
    _rememberPassword = existing?.rememberPassword ?? false;
    _masterPasswordProtected =
        widget.storedCredential?.masterPasswordProtected ??
        existing?.masterPasswordProtected ??
        false;
  }

  @override
  void dispose() {
    _name.dispose();
    _host.dispose();
    _port.dispose();
    _username.dispose();
    _secret.clear();
    _secret.dispose();
    _keyPath.dispose();
    _masterPassword.clear();
    _masterPassword.dispose();
    _masterPasswordConfirmation.clear();
    _masterPasswordConfirmation.dispose();
    super.dispose();
  }

  bool _connectionAvailable(ConnectionType type) => switch (type) {
    ConnectionType.sshTerminal => widget.capabilities?.ssh ?? false,
    ConnectionType.sftpOnly => widget.capabilities?.sftp ?? false,
    ConnectionType.remoteDesktop =>
      widget.capabilities?.supportsRemoteDesktop ?? false,
    ConnectionType.sshX11 => widget.capabilities?.sshX11 ?? false,
  };

  bool _authenticationAvailable(AuthenticationMethod method) =>
      switch (method) {
        AuthenticationMethod.password => widget.capabilities?.password ?? false,
        AuthenticationMethod.sshKey => widget.capabilities?.sshKey ?? false,
        AuthenticationMethod.smartCard =>
          (widget.capabilities?.smartCard ?? false) ||
              (widget.capabilities?.fido2 ?? false),
        AuthenticationMethod.mfa => widget.capabilities?.mfa ?? false,
      };

  void _submit(bool connect) {
    setState(() => _formMessage = null);
    if (!_formKey.currentState!.validate()) {
      _showFormMessage('Complete the highlighted required fields.');
      return;
    }
    if (!_connectionAvailable(_connectionType)) {
      _showFormMessage('${_connectionType.label} requires a backend.');
      return;
    }
    if (!_authenticationAvailable(_authentication)) {
      _showFormMessage('${_authentication.label} is not available.');
      return;
    }
    if (_rememberPassword &&
        _secret.text.isEmpty &&
        widget.storedCredential == null) {
      _showFormMessage('Enter a secret to remember it securely.');
      return;
    }
    if (_rememberPassword &&
        widget.storedCredential != null &&
        _secret.text.isEmpty &&
        _masterPasswordProtected !=
            widget.storedCredential!.masterPasswordProtected) {
      _showFormMessage(
        'Enter the credential again to change master-password protection.',
      );
      return;
    }
    if (_masterPasswordProtected && _secret.text.isNotEmpty) {
      if (_masterPassword.text.length < 10) {
        _showFormMessage(
          'Master password must contain at least 10 characters.',
        );
        return;
      }
      if (_masterPassword.text != _masterPasswordConfirmation.text) {
        _showFormMessage('Master password confirmation does not match.');
        return;
      }
    }
    final profile = SavedSession(
      id:
          widget.existing?.id ??
          DateTime.now().microsecondsSinceEpoch.toString(),
      name: _name.text.trim(),
      host: _host.text.trim(),
      port: int.parse(_port.text),
      username: _username.text.trim(),
      connectionType: _connectionType,
      authentication: _authentication,
      desktopEnvironment: _desktopEnvironment,
      keyPath: _authentication == AuthenticationMethod.sshKey
          ? _keyPath.text.trim()
          : null,
      wireguardConfigPath: _wireguardConfigPath,
      rememberUsername: _rememberUsername,
      rememberPassword: _rememberPassword,
      masterPasswordProtected: _rememberPassword && _masterPasswordProtected,
      favorite: widget.existing?.favorite ?? false,
    );
    Navigator.pop(
      context,
      _SessionRequest(
        profile: profile,
        secret: _secret.text,
        masterPassword: _masterPasswordProtected && _secret.text.isNotEmpty
            ? _masterPassword.text
            : null,
        save: _save,
        connect: connect,
      ),
    );
  }

  void _showFormMessage(String message) {
    if (mounted) setState(() => _formMessage = message);
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    return AlertDialog(
      title: Row(
        children: [
          const Icon(Icons.add_link_rounded),
          const SizedBox(width: 10),
          Text(
            widget.existing == null ? 'New Connection' : 'Connection Settings',
          ),
        ],
      ),
      content: SizedBox(
        width: 680,
        child: Form(
          key: _formKey,
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (_formMessage != null) ...[
                  AutoDismissBanner(
                    key: ValueKey(_formMessage),
                    message: _formMessage!,
                    leading: const Icon(Icons.error_outline_rounded),
                    onDismiss: () {
                      if (mounted) setState(() => _formMessage = null);
                    },
                  ),
                  const SizedBox(height: 14),
                ],
                const _FormSectionTitle('SERVER'),
                TextFormField(
                  controller: _name,
                  decoration: const InputDecoration(
                    labelText: 'Connection name',
                  ),
                  validator: _required,
                ),
                const SizedBox(height: 12),
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      flex: 4,
                      child: TextFormField(
                        controller: _host,
                        autofocus: true,
                        decoration: const InputDecoration(
                          labelText: 'Host',
                          hintText: 'server.example.com',
                        ),
                        validator: _required,
                      ),
                    ),
                    const SizedBox(width: 12),
                    SizedBox(
                      width: 110,
                      child: TextFormField(
                        controller: _port,
                        decoration: const InputDecoration(labelText: 'Port'),
                        keyboardType: TextInputType.number,
                        validator: _portValidator,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 18),
                const _FormSectionTitle('CONNECTION TYPE'),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: ConnectionType.values.map((type) {
                    final available = _connectionAvailable(type);
                    return ChoiceChip(
                      selected: _connectionType == type,
                      onSelected: available
                          ? (_) => setState(() => _connectionType = type)
                          : null,
                      avatar: Icon(_connectionIcon(type), size: 17),
                      label: Text(
                        '${type.label}${available ? '' : ' · backend required'}',
                      ),
                    );
                  }).toList(),
                ),
                if (_connectionType == ConnectionType.remoteDesktop) ...[
                  const SizedBox(height: 14),
                  DropdownButtonFormField<DesktopEnvironment>(
                    initialValue: _desktopEnvironment,
                    decoration: const InputDecoration(
                      labelText: 'Desktop environment',
                    ),
                    items: DesktopEnvironment.values
                        .map(
                          (environment) => DropdownMenuItem(
                            value: environment,
                            child: Text(environment.label),
                          ),
                        )
                        .toList(),
                    onChanged: (value) {
                      if (value != null) {
                        setState(() => _desktopEnvironment = value);
                      }
                    },
                  ),
                ],
                const SizedBox(height: 18),
                const _FormSectionTitle('AUTHENTICATION'),
                DropdownButtonFormField<AuthenticationMethod>(
                  initialValue: _authentication,
                  decoration: const InputDecoration(labelText: 'Method'),
                  items: AuthenticationMethod.values
                      .map(
                        (method) => DropdownMenuItem(
                          value: method,
                          enabled: _authenticationAvailable(method),
                          child: Text(
                            '${method.label}${_authenticationAvailable(method) ? '' : ' · backend required'}',
                          ),
                        ),
                      )
                      .toList(),
                  onChanged: (value) {
                    if (value != null && _authenticationAvailable(value)) {
                      setState(() => _authentication = value);
                    }
                  },
                ),
                const SizedBox(height: 12),
                TextFormField(
                  controller: _username,
                  decoration: const InputDecoration(labelText: 'Username'),
                  validator: _required,
                ),
                ListTile(
                  contentPadding: EdgeInsets.zero,
                  leading: const Icon(Icons.vpn_lock_outlined),
                  title: const Text('WireGuard for AetherDesk'),
                  subtitle: Text(
                    _wireguardConfigPath == null
                        ? 'Optional · connect to a private network'
                        : 'One active connection per config · desktop and SFTP share it',
                  ),
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      TextButton(
                        onPressed: () async {
                          final file = await openFile(
                            acceptedTypeGroups: [
                              const XTypeGroup(
                                label: 'WireGuard config',
                                extensions: ['conf'],
                              ),
                            ],
                          );
                          if (file != null && mounted)
                            setState(() => _wireguardConfigPath = file.path);
                        },
                        child: Text(
                          _wireguardConfigPath == null
                              ? 'Import .conf'
                              : 'Change',
                        ),
                      ),
                      if (_wireguardConfigPath != null)
                        IconButton(
                          tooltip: 'Disable WireGuard',
                          icon: const Icon(Icons.close),
                          onPressed: () =>
                              setState(() => _wireguardConfigPath = null),
                        ),
                    ],
                  ),
                ),
                if (_authentication == AuthenticationMethod.sshKey) ...[
                  const SizedBox(height: 12),
                  TextFormField(
                    controller: _keyPath,
                    decoration: const InputDecoration(
                      labelText: 'Private key path',
                      hintText: r'C:\Users\name\.ssh\id_ed25519',
                    ),
                    validator: _required,
                  ),
                ],
                const SizedBox(height: 12),
                TextFormField(
                  controller: _secret,
                  obscureText: _hideSecret,
                  decoration: InputDecoration(
                    labelText: _authentication == AuthenticationMethod.sshKey
                        ? 'Key passphrase (optional)'
                        : 'Password',
                    helperText: widget.storedCredential == null
                        ? 'Used only for this connection unless you choose secure storage.'
                        : 'Leave blank to keep the existing stored credential.',
                    suffixIcon: IconButton(
                      onPressed: () =>
                          setState(() => _hideSecret = !_hideSecret),
                      icon: Icon(
                        _hideSecret
                            ? Icons.visibility_outlined
                            : Icons.visibility_off_outlined,
                      ),
                    ),
                  ),
                ),
                const SizedBox(height: 18),
                const _FormSectionTitle('CREDENTIAL POLICY'),
                Container(
                  decoration: BoxDecoration(
                    color: colors.surfaceContainerHighest.withValues(
                      alpha: .45,
                    ),
                    borderRadius: BorderRadius.circular(10),
                    border: Border.all(color: colors.outlineVariant),
                  ),
                  child: Column(
                    children: [
                      CheckboxListTile(
                        value: _rememberUsername,
                        onChanged: (value) =>
                            setState(() => _rememberUsername = value ?? false),
                        title: const Text('Remember Username'),
                        subtitle: const Text(
                          'Saved in the connection profile.',
                        ),
                        controlAffinity: ListTileControlAffinity.leading,
                      ),
                      CheckboxListTile(
                        value: _rememberPassword,
                        onChanged: (value) => setState(() {
                          _rememberPassword = value ?? false;
                          if (!_rememberPassword) {
                            _masterPasswordProtected = false;
                          }
                        }),
                        title: const Text('Remember Password'),
                        subtitle: const Text(
                          'Encrypted with DPAPI and stored in Windows Credential Manager.',
                        ),
                        controlAffinity: ListTileControlAffinity.leading,
                      ),
                      CheckboxListTile(
                        value: !_rememberPassword,
                        onChanged: (value) => setState(() {
                          if (value ?? false) {
                            _rememberPassword = false;
                            _masterPasswordProtected = false;
                          }
                        }),
                        title: const Text('Ask Every Time'),
                        subtitle: const Text(
                          'Do not retain the password or passphrase.',
                        ),
                        controlAffinity: ListTileControlAffinity.leading,
                      ),
                      ListTile(
                        leading: const Icon(Icons.delete_forever_outlined),
                        title: const Text('Never Save Credentials'),
                        subtitle: const Text(
                          'Forget username and secret when this dialog closes.',
                        ),
                        trailing: OutlinedButton(
                          onPressed: () => setState(() {
                            _rememberUsername = false;
                            _rememberPassword = false;
                            _masterPasswordProtected = false;
                          }),
                          child: const Text('Select'),
                        ),
                      ),
                      if (_rememberPassword)
                        SwitchListTile(
                          value: _masterPasswordProtected,
                          onChanged: (value) =>
                              setState(() => _masterPasswordProtected = value),
                          title: const Text('Require master password'),
                          subtitle: const Text(
                            'Adds a PBKDF2-derived factor to Windows encryption.',
                          ),
                          secondary: const Icon(
                            Icons.enhanced_encryption_outlined,
                          ),
                        ),
                    ],
                  ),
                ),
                if (_rememberPassword && _masterPasswordProtected) ...[
                  const SizedBox(height: 12),
                  Row(
                    children: [
                      Expanded(
                        child: TextFormField(
                          controller: _masterPassword,
                          obscureText: _hideMaster,
                          decoration: InputDecoration(
                            labelText: 'Master password',
                            suffixIcon: IconButton(
                              onPressed: () =>
                                  setState(() => _hideMaster = !_hideMaster),
                              icon: Icon(
                                _hideMaster
                                    ? Icons.visibility_outlined
                                    : Icons.visibility_off_outlined,
                              ),
                            ),
                          ),
                        ),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: TextFormField(
                          controller: _masterPasswordConfirmation,
                          obscureText: true,
                          decoration: const InputDecoration(
                            labelText: 'Confirm master password',
                          ),
                        ),
                      ),
                    ],
                  ),
                ],
                const SizedBox(height: 8),
                CheckboxListTile(
                  contentPadding: EdgeInsets.zero,
                  value: _save,
                  onChanged: (value) => setState(() => _save = value ?? false),
                  title: const Text('Save connection profile'),
                  controlAffinity: ListTileControlAffinity.leading,
                ),
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        TextButton(onPressed: () => _submit(false), child: const Text('Save')),
        if (widget.connectByDefault)
          FilledButton.icon(
            onPressed: () => _submit(true),
            icon: const Icon(Icons.login_rounded, size: 18),
            label: const Text('Connect in New Tab'),
          ),
      ],
    );
  }

  String? _required(String? value) =>
      value == null || value.trim().isEmpty ? 'Required' : null;

  String? _portValidator(String? value) {
    final port = int.tryParse(value ?? '');
    return port == null || port < 1 || port > 65535 ? '1–65535' : null;
  }
}

class _CredentialPrompt extends StatefulWidget {
  const _CredentialPrompt({
    required this.profile,
    required this.initialUsername,
    this.storedSecret,
  });

  final SavedSession profile;
  final String initialUsername;
  final String? storedSecret;

  @override
  State<_CredentialPrompt> createState() => _CredentialPromptState();
}

class _CredentialPromptState extends State<_CredentialPrompt> {
  late final TextEditingController _username;
  late final TextEditingController _secret;
  bool _hidden = true;

  @override
  void initState() {
    super.initState();
    _username = TextEditingController(text: widget.initialUsername);
    _secret = TextEditingController();
  }

  @override
  void dispose() {
    _username.dispose();
    _secret.clear();
    _secret.dispose();
    super.dispose();
  }

  void _submit() {
    if (_username.text.trim().isEmpty) return;
    final secret = widget.storedSecret ?? _secret.text;
    if (widget.profile.authentication == AuthenticationMethod.password &&
        secret.isEmpty) {
      return;
    }
    Navigator.pop(
      context,
      _ConnectionInput(username: _username.text.trim(), secret: secret),
    );
  }

  @override
  Widget build(BuildContext context) {
    final key = widget.profile.authentication == AuthenticationMethod.sshKey;
    return AlertDialog(
      title: Text('Connect to ${widget.profile.name}'),
      content: SizedBox(
        width: 430,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: _username,
              autofocus: _username.text.isEmpty,
              decoration: const InputDecoration(labelText: 'Username'),
            ),
            if (widget.storedSecret == null) ...[
              const SizedBox(height: 12),
              TextField(
                controller: _secret,
                autofocus: _username.text.isNotEmpty,
                obscureText: _hidden,
                onSubmitted: (_) => _submit(),
                decoration: InputDecoration(
                  labelText: key ? 'Key passphrase (optional)' : 'Password',
                  suffixIcon: IconButton(
                    onPressed: () => setState(() => _hidden = !_hidden),
                    icon: Icon(
                      _hidden
                          ? Icons.visibility_outlined
                          : Icons.visibility_off_outlined,
                    ),
                  ),
                ),
              ),
            ],
            const SizedBox(height: 12),
            Row(
              children: [
                Icon(
                  widget.storedSecret == null
                      ? Icons.no_encryption_gmailerrorred_outlined
                      : Icons.verified_user_outlined,
                  size: 17,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    widget.storedSecret == null
                        ? 'Ask Every Time is enabled. This secret will not be saved.'
                        : 'The saved secret will be used. Only the username is required.',
                    style: const TextStyle(fontSize: 12),
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton.icon(
          onPressed: _submit,
          icon: const Icon(Icons.login_rounded),
          label: const Text('Connect'),
        ),
      ],
    );
  }
}

class _FormSectionTitle extends StatelessWidget {
  const _FormSectionTitle(this.label);
  final String label;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 8),
    child: Text(
      label,
      style: TextStyle(
        fontSize: 10,
        fontWeight: FontWeight.w800,
        letterSpacing: 1.1,
        color: Theme.of(context).colorScheme.primary,
      ),
    ),
  );
}

class _MenuLabel extends StatelessWidget {
  const _MenuLabel(this.icon, this.label);
  final IconData icon;
  final String label;

  @override
  Widget build(BuildContext context) => Row(
    children: [Icon(icon, size: 18), const SizedBox(width: 10), Text(label)],
  );
}

IconData _connectionIcon(ConnectionType type) => switch (type) {
  ConnectionType.sshTerminal => Icons.terminal_rounded,
  ConnectionType.remoteDesktop => Icons.desktop_windows_outlined,
  ConnectionType.sshX11 => Icons.open_in_new_rounded,
  ConnectionType.sftpOnly => Icons.folder_copy_outlined,
};
