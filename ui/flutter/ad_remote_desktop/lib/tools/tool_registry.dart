import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';

class ToolParameter {
  const ToolParameter({
    required this.name,
    required this.label,
    this.required = true,
    this.defaultValue,
  });
  final String name;
  final String label;
  final bool required;
  final String? defaultValue;
  Map<String, Object?> toJson() => {
    'name': name,
    'label': label,
    'required': required,
    if (defaultValue != null) 'defaultValue': defaultValue,
  };
}

/// A reusable PowerShell or POSIX shell tool. Installing never executes code.
class UserTool {
  const UserTool({
    required this.id,
    required this.title,
    required this.description,
    required this.version,
    required this.target,
    required this.script,
    this.parameters = const [],
    required this.revision,
  });
  final String id, title, description, version, target, script, revision;
  final List<ToolParameter> parameters;

  static UserTool fromJson(Map<String, dynamic> json, {String revision = ''}) {
    const fields = {
      'schemaVersion',
      'id',
      'title',
      'description',
      'version',
      'target',
      'script',
      'parameters',
    };
    if (json.keys.any((key) => !fields.contains(key)) ||
        json['schemaVersion'] != 1) {
      throw const FormatException(
        'Use tool manifest schemaVersion 1 and supported fields only.',
      );
    }
    String text(String key, int limit, {bool empty = false}) {
      final value = json[key];
      if (value is! String ||
          (!empty && value.trim().isEmpty) ||
          value.length > limit ||
          value.contains('\u0000')) {
        throw FormatException('Invalid tool $key.');
      }
      return value;
    }

    final id = text('id', 64);
    ToolRegistry.validateId(id);
    final version = text('version', 40);
    if (!RegExp(r'^\d+\.\d+\.\d+$').hasMatch(version)) {
      throw const FormatException('Tool version must have the form 1.0.0.');
    }
    final target = text('target', 7);
    if (target != 'windows' && target != 'ssh') {
      throw const FormatException('Tool target must be windows or ssh.');
    }
    final values = json['parameters'] ?? [];
    if (values is! List || values.length > 16) {
      throw const FormatException('A tool can have at most 16 parameters.');
    }
    final parameters = <ToolParameter>[];
    final names = <String>{};
    for (final value in values) {
      if (value is! Map ||
          value.keys.any(
            (key) =>
                !{'name', 'label', 'required', 'defaultValue'}.contains(key),
          )) {
        throw const FormatException('Invalid tool parameter.');
      }
      final name = value['name'];
      final label = value['label'];
      final required = value['required'] ?? true;
      final defaultValue = value['defaultValue'];
      if (name is! String ||
          !RegExp(r'^[A-Za-z][A-Za-z0-9_]{0,39}$').hasMatch(name) ||
          !names.add(name.toLowerCase()) ||
          label is! String ||
          label.trim().isEmpty ||
          label.length > 100 ||
          required is! bool ||
          (defaultValue != null &&
              (defaultValue is! String ||
                  defaultValue.length > 2048 ||
                  defaultValue.contains('\u0000')))) {
        throw const FormatException('Invalid or duplicate tool parameter.');
      }
      parameters.add(
        ToolParameter(
          name: name,
          label: label,
          required: required,
          defaultValue: defaultValue as String?,
        ),
      );
    }
    final tool = UserTool(
      id: id,
      title: text('title', 120),
      description: text('description', 2000, empty: true),
      version: version,
      target: target,
      script: text('script', 32768),
      parameters: List.unmodifiable(parameters),
      revision: revision,
    );
    if (utf8.encode(jsonEncode(tool.toJson())).length >
        ToolRegistry.maxManifestBytes) {
      throw const FormatException(
        'Tool manifest is too large (maximum 64 KiB).',
      );
    }
    return tool;
  }

  Map<String, Object?> toJson() => {
    'schemaVersion': 1,
    'id': id,
    'title': title,
    'description': description,
    'version': version,
    'target': target,
    'script': script,
    'parameters': parameters.map((value) => value.toJson()).toList(),
  };

  /// Inputs become quoted environment values, never interpolated into the script.
  String command(Map<String, String> arguments) {
    if (arguments.keys.any(
      (name) => !parameters.any((parameter) => parameter.name == name),
    )) {
      throw const FormatException('Unknown tool parameter.');
    }
    final declarations = <String>[];
    for (final parameter in parameters) {
      final value = arguments[parameter.name] ?? parameter.defaultValue ?? '';
      if ((parameter.required && value.isEmpty) ||
          value.length > 2048 ||
          value.contains('\u0000')) {
        throw FormatException('Enter a valid value for ${parameter.label}.');
      }
      final variable = 'AETHER_TOOL_${parameter.name}';
      if (target == 'windows') {
        declarations.add("\$env:$variable = '${value.replaceAll("'", "''")}';");
      } else {
        declarations.add(
          "$variable='${value.replaceAll("'", "'\\''")}'; export $variable;",
        );
      }
    }
    final command = [...declarations, script].join('\n');
    if (command.length > 65536)
      throw const FormatException('Tool inputs are too large.');
    return command;
  }
}

class ToolRegistry extends ChangeNotifier {
  ToolRegistry({required this.directory});
  static final shared = ToolRegistry(
    directory: Directory(
      '${Platform.environment['LOCALAPPDATA'] ?? Directory.systemTemp.path}${Platform.pathSeparator}AetherDesk${Platform.pathSeparator}tools',
    ),
  );
  static const maxManifestBytes = 64 * 1024;
  final Directory directory;
  List<UserTool> _tools = [];
  List<String> _invalidFiles = [];
  Future<void> _pending = Future.value();
  bool _disposed = false;
  List<UserTool> get tools => List.unmodifiable(_tools);
  List<String> get invalidFiles => List.unmodifiable(_invalidFiles);

  static void validateId(String id) {
    if (!RegExp(r'^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$').hasMatch(id) ||
        id.length > 64 ||
        RegExp(r'^(con|prn|aux|nul|com[1-9]|lpt[1-9])$').hasMatch(id)) {
      throw const FormatException(
        'Tool id must be a lowercase name such as project-summary.',
      );
    }
  }

  Future<T> _serialize<T>(Future<T> Function() action) {
    final result = _pending.then((_) => action());
    _pending = result.then<void>((_) {}, onError: (Object _, StackTrace __) {});
    return result;
  }

  File _file(String id) {
    validateId(id);
    return File('${directory.path}${Platform.pathSeparator}$id.json');
  }

  Future<UserTool> read(String id) async {
    final file = _file(id);
    if (await FileSystemEntity.type(file.path, followLinks: false) !=
        FileSystemEntityType.file) {
      throw StateError('The tool no longer exists or is not a regular file.');
    }
    if (await file.length() > maxManifestBytes)
      throw const FormatException('Tool manifest is too large.');
    final bytes = await file.readAsBytes();
    if (bytes.length > maxManifestBytes)
      throw const FormatException('Tool manifest is too large.');
    final decoded = jsonDecode(utf8.decode(bytes));
    if (decoded is! Map<String, dynamic>)
      throw const FormatException('Tool manifest must be an object.');
    final tool = UserTool.fromJson(
      decoded,
      revision: sha256.convert(bytes).toString(),
    );
    if (tool.id != id)
      throw const FormatException('Tool id does not match its filename.');
    return tool;
  }

  Future<void> reload() => _serialize(_reload);
  Future<void> _reload() async {
    final tools = <UserTool>[];
    final invalid = <String>[];
    if (await directory.exists()) {
      await for (final entry in directory.list(followLinks: false)) {
        final name = entry.uri.pathSegments.last;
        if (!name.endsWith('.json')) continue;
        try {
          tools.add(await read(name.substring(0, name.length - 5)));
        } catch (_) {
          invalid.add(name);
        }
      }
    }
    tools.sort(
      (a, b) => a.title.toLowerCase().compareTo(b.title.toLowerCase()),
    );
    _tools = tools;
    _invalidFiles = invalid;
    if (!_disposed) notifyListeners();
  }

  Future<void> _checkRevision(String id, String expected) async {
    final type = await FileSystemEntity.type(
      _file(id).path,
      followLinks: false,
    );
    if (expected == 'new' && type == FileSystemEntityType.notFound) return;
    if (expected == 'new' ||
        type != FileSystemEntityType.file ||
        (await read(id)).revision != expected) {
      throw StateError(
        'CONFLICT: the tool changed. Review it again before saving or running.',
      );
    }
  }

  Future<UserTool> install(
    Map<String, dynamic> manifest, {
    required String expectedRevision,
  }) => _serialize(() async {
    final tool = UserTool.fromJson(manifest);
    await directory.create(recursive: true);
    await _checkRevision(tool.id, expectedRevision);
    final file = _file(tool.id);
    final temporary = File(
      '${file.path}.${DateTime.now().microsecondsSinceEpoch}.tmp',
    );
    try {
      await temporary.writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(tool.toJson())}\n',
        flush: true,
      );
      await _checkRevision(tool.id, expectedRevision);
      await temporary.rename(file.path);
    } finally {
      if (await temporary.exists()) await temporary.delete();
    }
    await _reload();
    return read(tool.id);
  });

  Future<void> remove(String id, {required String expectedRevision}) =>
      _serialize(() async {
        await _checkRevision(id, expectedRevision);
        await _file(id).delete();
        await _reload();
      });

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
