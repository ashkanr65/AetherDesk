import 'dart:io';
import 'file_action_policy.dart';

class DocumentSession {
  DocumentSession({
    required this.id,
    required this.remotePath,
    required this.localFile,
    required this.metadata,
    this.remote = true,
    this.content,
  });
  final String id;
  String remotePath;
  final File localFile;
  Map<String, dynamic> metadata;
  final bool remote;
  TextFileContent? content;
  String get name => fileName(remotePath);
  String text = '';
  String savedText = '';
  String localDigest = '';
  String? externalObservedDigest;
  bool externalWatching = false;
  String status = 'Opening';
  String? error;
  bool external = false, dirty = false, saving = false;
  bool editorVisible = false;
  int generation = 0;
  Future<void>? saveFuture;
}
