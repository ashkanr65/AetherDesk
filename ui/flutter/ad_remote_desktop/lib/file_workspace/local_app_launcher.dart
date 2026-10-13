import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';

typedef _ShellNative =
    IntPtr Function(
      IntPtr,
      Pointer<Utf16>,
      Pointer<Utf16>,
      Pointer<Utf16>,
      Pointer<Utf16>,
      Int32,
    );
typedef _ShellDart =
    int Function(
      int,
      Pointer<Utf16>,
      Pointer<Utf16>,
      Pointer<Utf16>,
      Pointer<Utf16>,
      int,
    );

class LocalAppLauncher {
  const LocalAppLauncher();
  Future<void> open(String path, {String? application}) async {
    if (path.contains('\u0000'))
      throw const FileSystemException('Invalid file path');
    path = File(path).absolute.path;
    if (!await File(path).exists())
      throw FileSystemException('File was not found', path);
    if (application != null &&
        application.isNotEmpty &&
        application != 'system') {
      if (!await File(application).exists())
        throw FileSystemException(
          'Configured application was not found',
          application,
        );
      await Process.start(application, [path], mode: ProcessStartMode.detached);
      return;
    }
    if (!Platform.isWindows) {
      await Process.start('xdg-open', [path], mode: ProcessStartMode.detached);
      return;
    }
    if ([
      'exe',
      'com',
      'bat',
      'cmd',
      'msi',
      'scr',
      'ps1',
      'vbs',
      'js',
      'jse',
      'vbe',
      'hta',
      'lnk',
      'url',
      'wsf',
      'wsh',
      'msc',
      'cpl',
      'reg',
      'application',
      'appref-ms',
    ].contains(path.split('.').last.toLowerCase())) {
      throw const FileSystemException(
        'Choose an editor explicitly for executable or script files.',
      );
    }
    final verb = 'open'.toNativeUtf16();
    final file = path.toNativeUtf16();
    try {
      final result = DynamicLibrary.open('shell32.dll')
          .lookupFunction<_ShellNative, _ShellDart>(
            'ShellExecuteW',
          )(0, verb, file, nullptr, nullptr, 1);
      if (result <= 32)
        throw FileSystemException(
          'Windows could not open this file. Use Open with to choose an application.',
          path,
          OSError('ShellExecute failed', result),
        );
    } finally {
      calloc.free(verb);
      calloc.free(file);
    }
  }
}
