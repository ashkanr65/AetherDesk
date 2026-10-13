import 'package:flutter_test/flutter_test.dart';
import 'package:ad_remote_desktop/editor/editor_workspace.dart';

void main() {
  test('late flush snapshots cannot erase a host reload or newer edit', () {
    expect(editorSnapshotIsStale(5, 'new edit', 4, 'old edit'), true);
    expect(editorSnapshotIsStale(5, 'host reload', 5, 'old view'), true);
    expect(editorSnapshotIsStale(5, 'new edit', 5, 'new edit'), false);
    expect(editorSnapshotIsStale(5, 'new edit', 6, 'newest edit'), false);
  });
}
