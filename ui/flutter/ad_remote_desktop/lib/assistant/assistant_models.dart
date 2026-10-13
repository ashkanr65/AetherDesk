enum AssistantPhase { stopped, starting, ready, failed }

class AssistantModel {
  const AssistantModel({
    required this.id,
    required this.name,
    required this.providerId,
  });
  final String id;
  final String name;
  final String providerId;
  String get key => '$providerId/$id';
}

class AssistantProvider {
  const AssistantProvider({
    required this.id,
    required this.name,
    required this.connected,
    required this.models,
    this.baseUrl = '',
    this.requiresApiKey = true,
    this.description = '',
  });
  final String id;
  final String name;
  final bool connected;
  final List<AssistantModel> models;
  final String baseUrl, description;
  final bool requiresApiKey;
}

class AssistantConversation {
  const AssistantConversation({required this.id, required this.title});
  final String id;
  final String title;
}

class AssistantToolCall {
  const AssistantToolCall({
    required this.name,
    required this.status,
    required this.input,
    required this.output,
  });
  final String name;
  final String status;
  final Map<String, dynamic> input;
  final String output;
}

class AssistantMessage {
  const AssistantMessage({
    required this.id,
    required this.role,
    required this.text,
    this.tools = const [],
    this.images = const [],
    this.error,
  });
  final String id;
  final String role;
  final String text;
  final List<AssistantToolCall> tools;
  final List<String> images;
  final String? error;

  factory AssistantMessage.fromJson(Map<String, dynamic> value) {
    final info = Map<String, dynamic>.from(value['info'] as Map? ?? {});
    final parts = (value['parts'] as List? ?? []).whereType<Map>();
    final error = info['error'];
    return AssistantMessage(
      id: info['id'] as String? ?? '',
      role: info['role'] as String? ?? 'assistant',
      text: parts
          .where((part) => part['type'] == 'text')
          .map((part) => part['text'] ?? '')
          .join('\n'),
      images: [
        for (final part in parts)
          if (part['type'] == 'image' &&
              part['url'] is String &&
              (part['url'] as String).startsWith('data:image/png;base64,'))
            part['url'] as String,
      ],
      tools: [
        for (final part in parts.where((part) => part['type'] == 'tool'))
          AssistantToolCall(
            name: part['tool'] as String? ?? 'Tool',
            status: (part['state'] as Map?)?['status'] as String? ?? 'pending',
            input: Map<String, dynamic>.from(
              (part['state'] as Map?)?['input'] as Map? ?? {},
            ),
            output:
                ((part['state'] as Map?)?['output'] ??
                        (part['state'] as Map?)?['error'] ??
                        '')
                    .toString(),
          ),
      ],
      error: error is Map
          ? ((error['data'] as Map?)?['message'] ??
                    error['name'] ??
                    'The model request failed.')
                .toString()
          : null,
    );
  }
}

class AssistantApproval {
  const AssistantApproval({
    required this.id,
    required this.permission,
    required this.patterns,
    this.metadata = const {},
  });
  final String id;
  final String permission;
  final List<String> patterns;
  final Map<String, dynamic> metadata;
}

class AssistantQuestion {
  const AssistantQuestion({required this.id, required this.questions});
  final String id;
  final List<Map<String, dynamic>> questions;
}
