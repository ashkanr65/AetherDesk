import '../file_workspace/document_session.dart';
import '../file_workspace/file_open_controller.dart';

/// Serializable state shared by the owner and its separate editor engine.
/// Paths identify working copies; only the owner writes those copies or journals.
Map<String, dynamic> editorPopupSnapshot(
  FileOpenController controller, {
  int? revision,
  String? resultId,
}) => {
  if (revision != null) 'revision': revision,
  if (resultId != null) 'resultId': resultId,
  'selectedId': controller.selectedId,
  'documents': controller.documents.map(editorPopupDocument).toList(),
};

Map<String, dynamic> editorPopupDocument(DocumentSession doc) => {
  'id': doc.id,
  'remotePath': doc.remotePath,
  'localPath': doc.localFile.path,
  'remote': doc.remote,
  'metadata': doc.metadata,
  'content': doc.content == null
      ? null
      : {'encoding': doc.content!.encoding, 'bom': doc.content!.bom},
  'text': doc.text,
  'savedText': doc.savedText,
  'generation': doc.generation,
  'dirty': doc.dirty,
  'saving': doc.saving,
  'editorVisible': doc.editorVisible,
  'external': doc.external,
  'externalWatching': doc.externalWatching,
  'localDigest': doc.localDigest,
  'externalObservedDigest': doc.externalObservedDigest,
  'status': doc.status,
  'error': doc.error,
};
