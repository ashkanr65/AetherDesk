import 'dart:convert';
import 'dart:io';
import 'file_action_policy.dart';

Directory workspaceDataDirectory() => Directory(
  '${Platform.environment['LOCALAPPDATA'] ?? Directory.systemTemp.path}${Platform.pathSeparator}AetherDesk${Platform.pathSeparator}file-workspace',
);

class FileAssociations {
  FileAssociations({Directory? directory})
    : directory = directory ?? workspaceDataDirectory();
  final Directory directory;
  final Map<String, String> applications = {};
  static Future<void> _writes = Future.value();
  File get _file => File('${directory.path}/applications.json');
  Future<void> load() async {
    if (await _file.exists()) {
      try {
        applications.addAll(
          Map<String, String>.from(
            jsonDecode(await _file.readAsString()) as Map,
          ),
        );
      } on Object {
        /* A damaged record does not stop file browsing. */
      }
    }
  }

  Future<void> set(String extensionOrPath, String application) async {
    final value = extensionOrPath.toLowerCase();
    final extension =
        value.contains('/') ||
            value.contains(r'\') ||
            value.contains('.') && !value.startsWith('.')
        ? fileExtension(value)
        : value.startsWith('.')
        ? value.substring(1)
        : value;
    if (extension.contains(RegExp(r'[/\\\x00-\x1f]')))
      throw const FormatException('Invalid file extension');
    final previous = _writes.catchError((Object _) {});
    final operation = previous.then((_) async {
      await directory.create(recursive: true);
      await load();
      applications[extension] = application;
      final temporary = File('${_file.path}.tmp');
      await temporary.writeAsString(jsonEncode(applications), flush: true);
      await temporary.rename(_file.path);
    });
    _writes = operation;
    return operation;
  }

  String? forPath(String path) => applications[fileExtension(path)];
}
