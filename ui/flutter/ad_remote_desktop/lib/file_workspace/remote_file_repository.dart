import 'dart:async';
import '../native_bridge.dart';

typedef FileProgressCallback = void Function(int done, int total);

class FileJob {
  String? id;
  bool cancelled = false;
  Future<void> cancel() async {
    cancelled = true;
    final current = id;
    if (current == null) return;
    try {
      await NativeBridge.cancelFileJob(current);
    } on NativeException catch (error) {
      if (!error.toString().contains('not found')) rethrow;
    }
  }
}

class RemoteFileRepository {
  const RemoteFileRepository(this.session);
  final String session;
  Future<Map<String, dynamic>> execute(
    Map<String, Object?> request, {
    FileJob? job,
    FileProgressCallback? onProgress,
  }) async {
    job ??= FileJob();
    if (job.cancelled) throw StateError('File operation cancelled');
    final id = await NativeBridge.startFileJob(session, request);
    job.id = id;
    try {
      if (job.cancelled) await NativeBridge.cancelFileJob(id);
      while (true) {
        final state = await NativeBridge.pollFileJob(id);
        if (state['state'] == 'completed')
          return Map<String, dynamic>.from(state['result'] as Map);
        if (state['state'] == 'failed')
          throw NativeException(state['error'] as String);
        if (state['state'] != 'running')
          throw StateError('Unknown file operation state');
        onProgress?.call(
          (state['transferred'] as num?)?.toInt() ?? 0,
          (state['total'] as num?)?.toInt() ?? 0,
        );
        await Future<void>.delayed(const Duration(milliseconds: 80));
      }
    } finally {
      if (job.id == id) job.id = null;
    }
  }
}
