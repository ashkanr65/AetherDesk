import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';

// Layout follows SHELLEXECUTEINFOW in the Windows SDK.
final class _ShellInfo extends Struct {
  @Uint32()
  external int size;
  @Uint32()
  external int mask;
  external Pointer<Void> window;
  external Pointer<Utf16> verb;
  external Pointer<Utf16> file;
  external Pointer<Utf16> parameters;
  external Pointer<Utf16> directory;
  @Int32()
  external int show;
  external Pointer<Void> instance;
  external Pointer<Void> idList;
  external Pointer<Utf16> className;
  external Pointer<Void> classKey;
  @Uint32()
  external int hotKey;
  external Pointer<Void> icon;
  external Pointer<Void> process;
}

void showWindowsFileProperties(String path) {
  if (!Platform.isWindows)
    throw UnsupportedError(
      'Windows properties are unavailable on this platform.',
    );
  final info = calloc<_ShellInfo>();
  final verb = 'properties'.toNativeUtf16();
  final file = File(path).absolute.path.toNativeUtf16();
  try {
    info.ref
      ..size = sizeOf<_ShellInfo>()
      ..mask = 0x0000000c | 0x00000400
      ..verb = verb
      ..file = file
      ..show = 1;
    final call = DynamicLibrary.open('shell32.dll')
        .lookupFunction<
          Int32 Function(Pointer<_ShellInfo>),
          int Function(Pointer<_ShellInfo>)
        >('ShellExecuteExW');
    if (call(info) == 0)
      throw FileSystemException(
        'Windows could not show properties for this file',
        path,
      );
  } finally {
    calloc.free(file);
    calloc.free(verb);
    calloc.free(info);
  }
}
