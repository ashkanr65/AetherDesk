import 'package:ad_remote_desktop/app.dart';
import 'package:ad_remote_desktop/app_theme.dart';
import 'package:ad_remote_desktop/assistant/assistant_page.dart';
import 'package:ad_remote_desktop/assistant/assistant_controller.dart';
import 'package:ad_remote_desktop/notifications.dart';
import 'package:ad_remote_desktop/terminal_theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

const _model = AssistantModel(
  id: 'test-model',
  name: 'Test model',
  providerId: 'test',
);

class FakeAssistantController extends AssistantController {
  AssistantPhase testPhase = AssistantPhase.ready;
  bool busy = false;
  int starts = 0;
  int stops = 0;
  int shutdowns = 0;
  bool _closed = false;
  String? testError;
  String? testProgress;
  String path = r'C:\Projects';
  String? remote;
  AssistantModel model = _model;
  final sent = <String>[];
  final connections = <(String, String)>[];
  final removedProviders = <String>[];
  final deletedChats = <String>[];
  int clearedHistory = 0;
  @override
  Future<void> deleteConversation(String id) async {
    deletedChats.add(id);
  }

  @override
  Future<void> clearHistory() async {
    clearedHistory++;
  }

  final approvalScopes = <String>[];
  @override
  Future<void> removeProvider(String id) async {
    removedProviders.add(id);
  }

  String? connectedUrl, connectedModel;
  final replies = <(String, bool)>[];
  final answers = <List<List<String>>>[];
  final selectedConversations = <String>[];
  final testMessages = <AssistantMessage>[];
  final testApprovals = <AssistantApproval>[];
  final testQuestions = <AssistantQuestion>[];
  final testHosts = <AssistantRemoteHost>[
    const AssistantRemoteHost(
      id: 'ssh-1',
      label: 'Design server',
      directory: '/home/designer',
    ),
  ];

  @override
  AssistantPhase get phase => testPhase;
  @override
  bool get isBusy => busy;
  @override
  String? get error => testError;
  @override
  String? get progress => testProgress;
  @override
  String get workspacePath => path;
  @override
  String? get remoteSessionId => remote;
  @override
  List<AssistantRemoteHost> get remoteHosts => testHosts;
  @override
  AssistantModel? get selectedModel => model;
  @override
  void selectModel(AssistantModel selected) {
    model = selected;
    notifyListeners();
  }

  @override
  List<AssistantProvider> get providers => testProviders;
  List<AssistantProvider> testProviders = const [
    AssistantProvider(
      id: 'test',
      name: 'Test provider',
      connected: true,
      models: [_model],
    ),
  ];
  @override
  List<AssistantConversation> get conversations => const [
    AssistantConversation(id: 'conversation-1', title: 'Review project'),
  ];
  @override
  String? get currentSessionId => null;
  @override
  List<AssistantMessage> get messages => testMessages;
  @override
  List<AssistantApproval> get approvals => testApprovals;
  @override
  List<AssistantQuestion> get questions => testQuestions;
  @override
  Future<void> start() async {
    starts++;
    testPhase = AssistantPhase.ready;
    testError = null;
    notifyListeners();
  }

  @override
  Future<void> send(String text, {Map<String, dynamic>? capture}) async {
    sent.add(text);
    testMessages.add(AssistantMessage(id: 'user-1', role: 'user', text: text));
    busy = true;
    notifyListeners();
  }

  @override
  Future<void> abort() async {
    stops++;
    busy = false;
    notifyListeners();
  }

  @override
  Future<void> connectProvider(
    String providerId,
    String apiKey, {
    String? baseUrl,
    String? modelId,
  }) async {
    connections.add((providerId, apiKey));
    connectedUrl = baseUrl;
    connectedModel = modelId;
  }

  @override
  Future<void> chooseWorkspace(String path, {String? remoteSessionId}) async {
    if (remoteSessionId == null) {
      this.path = path;
    } else {
      final index = testHosts.indexWhere((host) => host.id == remoteSessionId);
      final previous = testHosts[index];
      testHosts[index] = AssistantRemoteHost(
        id: previous.id,
        label: previous.label,
        identity: previous.identity,
        directory: path,
      );
    }
    remote = remoteSessionId;
    notifyListeners();
  }

  @override
  Future<void> replyPermission(
    String id,
    bool allow, {
    String scope = 'once',
  }) async {
    replies.add((id, allow));
    approvalScopes.add(scope);
    testApprovals.removeWhere((approval) => approval.id == id);
    notifyListeners();
  }

  @override
  Future<void> replyQuestion(String id, List<List<String>> answers) async {
    this.answers.add(answers);
    testQuestions.removeWhere((question) => question.id == id);
    notifyListeners();
  }

  @override
  Future<void> rejectQuestion(String id) async {
    testQuestions.removeWhere((question) => question.id == id);
    notifyListeners();
  }

  @override
  Future<void> selectConversation(String id) async {
    selectedConversations.add(id);
    notifyListeners();
  }

  @override
  Future<void> shutdown() async {
    if (!_closed) shutdowns++;
    _closed = true;
  }
}

void main() {
  late FakeAssistantController controller;
  setUp(() => controller = FakeAssistantController());
  tearDown(() => controller.dispose());

  Future<void> mount(WidgetTester tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: AssistantPage(controller: controller)),
      ),
    );
    await tester.pump();
  }

  testWidgets('sends a prompt and can stop the running assistant', (
    tester,
  ) async {
    await mount(tester);
    await tester.enterText(
      find.byKey(const Key('assistant-prompt')),
      'Explain this project',
    );
    await tester.tap(find.byKey(const Key('assistant-send')));
    await tester.pump();
    expect(controller.sent, ['Explain this project']);
    expect(find.text('Explain this project'), findsOneWidget);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('assistant-prompt')))
          .controller!
          .text,
      isEmpty,
    );
    await tester.tap(find.byKey(const Key('assistant-stop')));
    await tester.pump();
    expect(controller.stops, 1);
    expect(find.byKey(const Key('assistant-send')), findsOneWidget);
  });

  testWidgets('provider retry is visible and Stop stays available', (
    tester,
  ) async {
    controller.busy = true;
    controller.testProgress =
        'Provider busy (HTTP 429). Retrying in 2 seconds…';
    await mount(tester);
    expect(find.text(controller.testProgress!), findsOneWidget);
    expect(find.text('Assistant is working…'), findsNothing);
    await tester.tap(find.byKey(const Key('assistant-stop')));
    await tester.pump();
    expect(controller.stops, 1);
    expect(find.text(controller.testProgress!), findsNothing);
    expect(find.byKey(const Key('assistant-send')), findsOneWidget);
  });

  testWidgets('sent screen keeps its decoded image through chat refreshes', (
    tester,
  ) async {
    const url =
        'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=';
    controller.testMessages.add(
      const AssistantMessage(
        id: 'screen-message',
        role: 'user',
        text: 'Check this',
        images: [url],
      ),
    );
    await mount(tester);
    final initial =
        tester.widget<Image>(find.byType(Image)).image as MemoryImage;
    for (var i = 0; i < 5; i++) {
      controller.testMessages[0] = const AssistantMessage(
        id: 'screen-message',
        role: 'user',
        text: 'Check this',
        images: [url],
      );
      controller.notifyListeners();
      await tester.pump(const Duration(seconds: 3));
      final refreshed = tester.widget<Image>(find.byType(Image));
      expect(
        identical((refreshed.image as MemoryImage).bytes, initial.bytes),
        isTrue,
      );
      expect(refreshed.gaplessPlayback, isTrue);
    }
    expect(tester.takeException(), isNull);
  });

  testWidgets('Enter sends and Ctrl+Enter inserts a newline at the caret', (
    tester,
  ) async {
    await mount(tester);
    final prompt = find.byKey(const Key('assistant-prompt'));
    await tester.enterText(prompt, 'firstsecond');
    final field = tester.widget<TextField>(prompt).controller!;
    field.selection = const TextSelection.collapsed(offset: 5);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await tester.pump();
    expect(field.text, 'first\nsecond');
    expect(controller.sent, isEmpty);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(controller.sent, ['first\nsecond']);
    expect(field.text, isEmpty);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(controller.sent.length, 1);
  });

  testWidgets('changes from Windows workspace to a connected SSH host', (
    tester,
  ) async {
    await mount(tester);
    await tester.tap(find.byKey(const Key('assistant-workspace')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('assistant-source')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Design server').last);
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('assistant-workspace-path')))
          .controller!
          .text,
      '/home/designer',
    );
    await tester.enterText(
      find.byKey(const Key('assistant-workspace-path')),
      '/home/designer/chip',
    );
    await tester.tap(find.text('Use workspace'));
    await tester.pumpAndSettle();
    expect(controller.remote, 'ssh-1');
    expect(controller.remoteHosts.single.directory, '/home/designer/chip');
    expect(find.text('Design server · /home/designer/chip'), findsOneWidget);
  });

  testWidgets('remote workspace displays and edits its remote folder', (
    tester,
  ) async {
    controller.remote = 'ssh-1';
    await mount(tester);
    expect(find.text('Design server · /home/designer'), findsOneWidget);
    await tester.tap(find.byKey(const Key('assistant-workspace')));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('assistant-workspace-path')))
          .controller!
          .text,
      '/home/designer',
    );
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
  });

  testWidgets('approval buttons send the exact pending request and decision', (
    tester,
  ) async {
    controller.testApprovals.addAll(const [
      AssistantApproval(
        id: 'allow-1',
        permission: 'write file',
        patterns: ['design.v'],
      ),
      AssistantApproval(
        id: 'deny-1',
        permission: 'run command',
        patterns: ['remove build'],
      ),
    ]);
    await mount(tester);
    await tester.tap(find.text('Allow this message').first);
    await tester.pump();
    await tester.tap(find.text('Deny'));
    await tester.pump();
    expect(controller.replies, [('allow-1', true), ('deny-1', false)]);
    expect(controller.approvals, isEmpty);
    expect(controller.approvalScopes.first, 'message');
  });

  testWidgets('answers agent questions with an option and custom text', (
    tester,
  ) async {
    controller.testQuestions.add(
      const AssistantQuestion(
        id: 'question-1',
        questions: [
          {
            'question': 'Which language?',
            'options': [
              {'label': 'Dart', 'description': 'Flutter UI'},
              {'label': 'Rust', 'description': 'Core'},
            ],
          },
          {'question': 'What should change?', 'options': []},
        ],
      ),
    );
    await mount(tester);
    await tester.tap(find.widgetWithText(FilterChip, 'Dart'));
    await tester.pump();
    await tester.enterText(
      find.widgetWithText(TextField, 'Your answer').last,
      'Improve startup',
    );
    await tester.pump();
    await tester.tap(find.text('Answer'));
    await tester.pump();
    expect(controller.answers, [
      [
        ['Dart'],
        ['Improve startup'],
      ],
    ]);
    expect(controller.questions, isEmpty);
  });

  testWidgets('failed startup offers retry and displays the failure', (
    tester,
  ) async {
    controller.testPhase = AssistantPhase.failed;
    controller.testError = 'Use the complete AetherDesk package.';
    await mount(tester);
    expect(find.text('Use the complete AetherDesk package.'), findsOneWidget);
    await tester.tap(find.text('Try again'));
    await tester.pump();
    expect(controller.starts, 1);
    expect(find.byKey(const Key('assistant-prompt')), findsOneWidget);
  });

  testWidgets('sidebar opens a selected conversation', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(body: AssistantHistory(controller: controller)),
      ),
    );
    await tester.tap(find.text('Review project'));
    await tester.pump();
    expect(controller.selectedConversations, ['conversation-1']);
  });

  testWidgets(
    'provider setup obscures the key and passes it to the controller',
    (tester) async {
      await mount(tester);
      await tester.tap(find.byTooltip('Manage providers'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('assistant-provider')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Test provider').last);
      await tester.pumpAndSettle();
      final apiKey = find.widgetWithText(TextField, 'API key');
      expect(tester.widget<TextField>(apiKey).obscureText, isTrue);
      await tester.enterText(apiKey, 'fixture-api-key');
      await tester.pump();
      await tester.tap(find.text('Connect'));
      await tester.pumpAndSettle();
      expect(controller.connections, [('test', 'fixture-api-key')]);
      expect(find.text('Connect an AI provider'), findsNothing);
    },
  );

  testWidgets('custom API accepts a local URL and manual model without a key', (
    tester,
  ) async {
    controller.testProviders = const [
      AssistantProvider(
        id: 'custom',
        name: 'Compatible API',
        connected: false,
        models: [],
        requiresApiKey: false,
      ),
    ];
    await mount(tester);
    await tester.tap(find.byTooltip('Manage providers'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('assistant-provider')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Compatible API'));
    await tester.pumpAndSettle();
    expect(
      find.widgetWithText(TextField, 'API key (optional)'),
      findsOneWidget,
    );
    await tester.enterText(
      find.byKey(const Key('assistant-provider-url')),
      'http://127.0.0.1:1234/v1',
    );
    await tester.enterText(
      find.byKey(const Key('assistant-provider-model')),
      'local-code-model',
    );
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await tester.pumpAndSettle();
    expect(controller.connections, [('custom', '')]);
    expect(controller.connectedUrl, 'http://127.0.0.1:1234/v1');
    expect(controller.connectedModel, 'local-code-model');
  });

  const searchableProviders = [
    AssistantProvider(
      id: 'z-provider',
      name: 'Zulu Provider',
      connected: true,
      models: [
        AssistantModel(
          id: 'z-model',
          name: 'Zulu Model',
          providerId: 'z-provider',
        ),
        AssistantModel(
          id: 'alpha-model',
          name: 'alpha Code',
          providerId: 'z-provider',
        ),
      ],
    ),
    AssistantProvider(
      id: 'alpha-provider',
      name: 'alpha Provider',
      connected: true,
      models: [
        AssistantModel(
          id: 'internal-beta',
          name: 'Beta Chat',
          providerId: 'alpha-provider',
        ),
      ],
    ),
    AssistantProvider(
      id: 'offline',
      name: 'Bravo Provider',
      connected: false,
      models: [
        AssistantModel(
          id: 'hidden',
          name: 'Hidden Model',
          providerId: 'offline',
        ),
      ],
    ),
  ];

  List<String> visibleChoices(WidgetTester tester) => tester
      .widgetList<ListTile>(
        find.byWidgetPredicate(
          (widget) =>
              widget is ListTile &&
              widget.key is ValueKey<String> &&
              (widget.key! as ValueKey<String>).value.startsWith(
                'assistant-choice-',
              ),
        ),
      )
      .map((tile) => (tile.title! as Text).data!)
      .toList();

  testWidgets('model picker sorts A-Z and searches names, IDs and providers', (
    tester,
  ) async {
    controller.testProviders = searchableProviders;
    await mount(tester);
    await tester.tap(find.byKey(const Key('assistant-model')));
    await tester.pumpAndSettle();
    expect(visibleChoices(tester), ['alpha Code', 'Beta Chat', 'Zulu Model']);
    final search = find.byKey(const Key('assistant-selection-search'));
    expect(
      tester.getRect(search).bottom,
      lessThan(tester.getRect(find.byType(ListTile).first).top),
    );
    await tester.enterText(search, 'missing model');
    await tester.pump();
    expect(find.text('No matches found.'), findsOneWidget);
    expect(controller.model, same(_model));
    await tester.tap(find.byTooltip('Clear search'));
    await tester.pump();
    expect(visibleChoices(tester), ['alpha Code', 'Beta Chat', 'Zulu Model']);
    await tester.enterText(search, '  ALPHA PROVIDER internal-beta  ');
    await tester.pump();
    expect(visibleChoices(tester), ['Beta Chat']);
    await tester.tap(
      find.byKey(const Key('assistant-choice-alpha-provider/internal-beta')),
    );
    await tester.pumpAndSettle();
    expect(controller.model.key, 'alpha-provider/internal-beta');
    expect(find.text('Choose model'), findsNothing);
    await tester.tap(find.byKey(const Key('assistant-model')));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<ListTile>(
            find.byKey(
              const Key('assistant-choice-alpha-provider/internal-beta'),
            ),
          )
          .selected,
      isTrue,
    );
    await tester.enterText(search, 'Zulu');
    await tester.pump();
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(controller.model.key, 'alpha-provider/internal-beta');
  });

  testWidgets(
    'provider key menu removes an existing API without entering a key',
    (tester) async {
      await mount(tester);
      expect(find.text('Connect provider'), findsNothing);
      expect(find.text('Workspace'), findsOneWidget);
      await tester.tap(find.byTooltip('Manage providers'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('assistant-provider')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('assistant-choice-test')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('assistant-remove-provider')));
      await tester.pumpAndSettle();
      expect(controller.removedProviders, ['test']);
      expect(controller.connections, isEmpty);
    },
  );

  testWidgets(
    'connected services are sorted and can be used or removed directly',
    (tester) async {
      controller.testProviders = searchableProviders;
      await mount(tester);
      await tester.tap(find.byTooltip('Manage providers'));
      await tester.pumpAndSettle();
      expect(find.text('Connected services'), findsOneWidget);
      final alpha = find.byKey(const Key('assistant-connected-alpha-provider'));
      final zulu = find.byKey(const Key('assistant-connected-z-provider'));
      expect(alpha, findsOneWidget);
      expect(zulu, findsOneWidget);
      expect(
        find.byKey(const Key('assistant-connected-offline')),
        findsNothing,
      );
      expect(tester.getTopLeft(alpha).dy, lessThan(tester.getTopLeft(zulu).dy));
      await tester.tap(alpha);
      await tester.pumpAndSettle();
      expect(find.widgetWithText(AlertDialog, 'Choose model'), findsOneWidget);
      expect(find.text('Beta Chat'), findsOneWidget);
      expect(find.text('Zulu Model'), findsNothing);
      await tester.tap(
        find.byKey(const Key('assistant-choice-alpha-provider/internal-beta')),
      );
      await tester.pumpAndSettle();
      expect(controller.model.key, 'alpha-provider/internal-beta');
      expect(controller.connections, isEmpty);
      await tester.tap(find.byTooltip('Manage providers'));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byKey(const Key('assistant-edit-service-z-provider')),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('assistant-provider-url')), findsOneWidget);
      expect(
        tester
            .widget<TextField>(find.widgetWithText(TextField, 'API key'))
            .controller!
            .text,
        isEmpty,
      );
      await tester.tap(
        find.byKey(const Key('assistant-remove-service-z-provider')),
      );
      await tester.pumpAndSettle();
      expect(controller.removedProviders, ['z-provider']);
      expect(controller.connections, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'history menu deletes selected chats and confirms clearing everything',
    (tester) async {
      await mount(tester);
      await tester.tap(find.byTooltip('Conversation history'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('delete-chat-conversation-1')));
      await tester.pumpAndSettle();
      expect(controller.deletedChats, isEmpty);
      await tester.tap(find.text('Delete conversation'));
      await tester.pumpAndSettle();
      expect(controller.deletedChats, ['conversation-1']);
      await tester.tap(find.byTooltip('Conversation history'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Clear all history'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(controller.clearedHistory, 0);
      await tester.tap(find.byTooltip('Conversation history'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Clear all history'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Clear history'));
      await tester.pumpAndSettle();
      expect(controller.clearedHistory, 1);
    },
  );

  testWidgets('provider picker sorts A-Z and filters before connecting', (
    tester,
  ) async {
    controller.testProviders = searchableProviders;
    await mount(tester);
    await tester.tap(find.byTooltip('Manage providers'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('assistant-provider')));
    await tester.pumpAndSettle();
    expect(visibleChoices(tester), [
      'alpha Provider',
      'Bravo Provider',
      'Zulu Provider',
    ]);
    final search = find.byKey(const Key('assistant-selection-search'));
    await tester.enterText(search, 'OFFLINE');
    await tester.pump();
    expect(visibleChoices(tester), ['Bravo Provider']);
    await tester.tap(find.byTooltip('Clear search'));
    await tester.pump();
    await tester.enterText(search, 'zULU');
    await tester.pump();
    expect(visibleChoices(tester), ['Zulu Provider']);
    await tester.tap(find.byKey(const Key('assistant-choice-z-provider')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.widgetWithText(TextField, 'API key'),
      'fixture-key',
    );
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await tester.pumpAndSettle();
    expect(controller.connections, [('z-provider', 'fixture-key')]);
  });

  testWidgets(
    'assistant stays in the sidebar without replacing the active workspace',
    (tester) async {
      tester.view.physicalSize = const Size(1600, 1100);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final theme = TerminalThemeManager();
      final appController = FakeAssistantController();
      var creations = 0;
      await tester.runAsync(() async {
        await tester.pumpWidget(
          MaterialApp(
            theme: aetherDeskTheme(Brightness.dark),
            home: AetherNoticeHost(
              child: AetherDeskHome(
                themeMode: ThemeMode.dark,
                terminalThemeManager: theme,
                onToggleTheme: () {},
                assistantControllerFactory: (_) {
                  creations++;
                  return appController;
                },
              ),
            ),
          ),
        );
      });
      await tester.pump();
      expect(creations, 0);
      expect(find.byTooltip('Port Forwards'), findsNothing);
      expect(find.byTooltip('SFTP Explorer'), findsNothing);
      final workspace = find.byKey(const Key('main-workspace'));
      final originalWorkspace = tester.element(workspace);
      final originalBounds = tester.getRect(workspace);
      await tester.tap(find.byTooltip('AI Assistant'));
      await tester.pump();
      expect(creations, 1);
      expect(appController.starts, 1);
      final selected = tester.widget<IconButton>(
        find.byWidgetPredicate(
          (widget) => widget is IconButton && widget.tooltip == 'AI Assistant',
        ),
      );
      final states = {WidgetState.selected};
      final foreground = selected.style!.foregroundColor!.resolve(states)!;
      final background = selected.style!.backgroundColor!.resolve(states)!;
      expect(foreground, const Color(0xFF2C2B3F));
      expect(background, const Color(0xFFC7A2B3));
      final contrast =
          (background.computeLuminance() + .05) /
          (foreground.computeLuminance() + .05);
      expect(contrast, greaterThanOrEqualTo(4.5));
      expect(tester.element(workspace), same(originalWorkspace));
      expect(tester.getRect(workspace), originalBounds);
      expect(workspace.hitTestable(), findsOneWidget);
      final assistantBounds = tester.getRect(find.byType(AssistantPage));
      final sidebarBounds = tester.getRect(
        find.byKey(const Key('workspace-sidebar')),
      );
      expect(sidebarBounds.contains(assistantBounds.center), isTrue);
      expect(assistantBounds.right, lessThanOrEqualTo(originalBounds.left));
      expect(tester.takeException(), isNull);
      final resize = find.byKey(const Key('workspace-panel-resize'));
      await tester.drag(resize, const Offset(170, 0));
      await tester.pump();
      expect(
        tester.getSize(find.byKey(const Key('workspace-sidebar'))).width,
        greaterThan(sidebarBounds.width + 120),
      );
      expect(tester.element(workspace), same(originalWorkspace));
      expect(tester.takeException(), isNull);
      final beforeRapidDrag = tester
          .getSize(find.byKey(const Key('workspace-sidebar')))
          .width;
      final drag = await tester.startGesture(tester.getCenter(resize));
      await drag.moveBy(const Offset(30, 0));
      for (var i = 0; i < 8; i++) {
        await drag.moveBy(const Offset(20, 0));
      }
      await tester.pump();
      expect(
        tester.getSize(find.byKey(const Key('workspace-sidebar'))).width,
        closeTo(beforeRapidDrag + 190, 1),
      );
      await drag.moveBy(const Offset(-50, 0));
      await tester.pump();
      expect(
        tester.getSize(find.byKey(const Key('workspace-sidebar'))).width,
        closeTo(beforeRapidDrag + 140, 1),
      );
      await drag.up();
      await tester.tap(find.byTooltip('Conversation history'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Review project'));
      await tester.pumpAndSettle();
      expect(appController.selectedConversations, ['conversation-1']);
      expect(tester.element(workspace), same(originalWorkspace));
      await tester.enterText(
        find.byKey(const Key('assistant-prompt')),
        'Keep my draft',
      );
      await tester.tap(find.byTooltip('Sessions'));
      await tester.pump();
      expect(find.byKey(const Key('assistant-prompt')), findsNothing);
      await tester.tap(find.byTooltip('AI Assistant'));
      await tester.pump();
      expect(creations, 1);
      expect(appController.starts, 1);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('assistant-prompt')))
            .controller!
            .text,
        'Keep my draft',
      );
      await tester.tap(find.byTooltip('Toggle workspace panel (Ctrl+Shift+B)'));
      await tester.pump();
      expect(find.byKey(const Key('assistant-prompt')), findsNothing);
      expect(tester.element(workspace), same(originalWorkspace));
      await tester.tap(find.byTooltip('Toggle workspace panel (Ctrl+Shift+B)'));
      await tester.pump();
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('assistant-prompt')))
            .controller!
            .text,
        'Keep my draft',
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pump();
      expect(appController.shutdowns, 1);
      theme.dispose();
    },
  );
}
