import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';
import 'dart:math';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:ffi/ffi.dart';

const _credentialTypeGeneric = 1;
const _credentialTypeDomainPassword = 2;
const _credentialPersistSession = 1;
const _credentialPersistLocalMachine = 2;
const _cryptProtectUiForbidden = 0x1;
const _errorNotFound = 1168;
const _pbkdf2Iterations = 210000;

final class _FileTime extends Struct {
  @Uint32()
  external int lowDateTime;

  @Uint32()
  external int highDateTime;
}

final class _CredentialW extends Struct {
  @Uint32()
  external int flags;

  @Uint32()
  external int type;

  external Pointer<Utf16> targetName;
  external Pointer<Utf16> comment;
  external _FileTime lastWritten;

  @Uint32()
  external int credentialBlobSize;

  external Pointer<Uint8> credentialBlob;

  @Uint32()
  external int persist;

  @Uint32()
  external int attributeCount;

  external Pointer<Void> attributes;
  external Pointer<Utf16> targetAlias;
  external Pointer<Utf16> userName;
}

final class _DataBlob extends Struct {
  @Uint32()
  external int length;

  external Pointer<Uint8> data;
}

typedef _CredWriteNative = Int32 Function(Pointer<_CredentialW>, Uint32);
typedef _CredWriteDart = int Function(Pointer<_CredentialW>, int);
typedef _CredReadNative =
    Int32 Function(
      Pointer<Utf16>,
      Uint32,
      Uint32,
      Pointer<Pointer<_CredentialW>>,
    );
typedef _CredReadDart =
    int Function(Pointer<Utf16>, int, int, Pointer<Pointer<_CredentialW>>);
typedef _CredDeleteNative = Int32 Function(Pointer<Utf16>, Uint32, Uint32);
typedef _CredDeleteDart = int Function(Pointer<Utf16>, int, int);
typedef _CredFreeNative = Void Function(Pointer<Void>);
typedef _CredFreeDart = void Function(Pointer<Void>);
typedef _GetLastErrorNative = Uint32 Function();
typedef _GetLastErrorDart = int Function();
typedef _CryptProtectNative =
    Int32 Function(
      Pointer<_DataBlob>,
      Pointer<Utf16>,
      Pointer<_DataBlob>,
      Pointer<Void>,
      Pointer<Void>,
      Uint32,
      Pointer<_DataBlob>,
    );
typedef _CryptProtectDart =
    int Function(
      Pointer<_DataBlob>,
      Pointer<Utf16>,
      Pointer<_DataBlob>,
      Pointer<Void>,
      Pointer<Void>,
      int,
      Pointer<_DataBlob>,
    );
typedef _CryptUnprotectNative =
    Int32 Function(
      Pointer<_DataBlob>,
      Pointer<Pointer<Utf16>>,
      Pointer<_DataBlob>,
      Pointer<Void>,
      Pointer<Void>,
      Uint32,
      Pointer<_DataBlob>,
    );
typedef _CryptUnprotectDart =
    int Function(
      Pointer<_DataBlob>,
      Pointer<Pointer<Utf16>>,
      Pointer<_DataBlob>,
      Pointer<Void>,
      Pointer<Void>,
      int,
      Pointer<_DataBlob>,
    );
typedef _LocalFreeNative = Pointer<Void> Function(Pointer<Void>);
typedef _LocalFreeDart = Pointer<Void> Function(Pointer<Void>);

class StoredCredential {
  const StoredCredential({required this.username, required this.secret});

  final String username;
  final String secret;
}

class CredentialMetadata {
  const CredentialMetadata({
    required this.username,
    required this.masterPasswordProtected,
  });

  final String username;
  final bool masterPasswordProtected;
}

class CredentialVault {
  const CredentialVault();

  static String targetFor(String profileId) => 'AetherDesk/$profileId';

  Future<void> store({
    required String profileId,
    required String username,
    required String secret,
    String? masterPassword,
  }) {
    return Isolate.run(
      () => _WindowsCredentialVault().store(
        target: targetFor(profileId),
        username: username,
        secret: secret,
        masterPassword: masterPassword,
      ),
    );
  }

  Future<CredentialMetadata?> inspect(String profileId) {
    return Isolate.run(
      () => _WindowsCredentialVault().inspect(targetFor(profileId)),
    );
  }

  Future<StoredCredential?> read(String profileId, {String? masterPassword}) {
    return Isolate.run(
      () => _WindowsCredentialVault().read(
        targetFor(profileId),
        masterPassword: masterPassword,
      ),
    );
  }

  Future<void> updateUsername(String profileId, String username) {
    return Isolate.run(
      () => _WindowsCredentialVault().updateUsername(
        targetFor(profileId),
        username,
      ),
    );
  }

  Future<void> storeTemporaryRemoteDesktopCredential({
    required String targetHost,
    required String username,
    required String password,
  }) {
    return Isolate.run(
      () => _WindowsCredentialVault().storeTemporaryRemoteDesktopCredential(
        targetHost: targetHost,
        username: username,
        password: password,
      ),
    );
  }

  Future<void> deleteTemporaryRemoteDesktopCredential(String targetHost) {
    return Isolate.run(
      () => _WindowsCredentialVault().deleteTemporaryRemoteDesktopCredential(
        targetHost,
      ),
    );
  }

  Future<void> delete(String profileId) {
    return Isolate.run(
      () => _WindowsCredentialVault().delete(targetFor(profileId)),
    );
  }
}

class CredentialVaultException implements Exception {
  const CredentialVaultException(this.message);

  final String message;

  @override
  String toString() => message;
}

class MasterPasswordRequiredException extends CredentialVaultException {
  const MasterPasswordRequiredException()
    : super('This credential is protected by a master password.');
}

class InvalidMasterPasswordException extends CredentialVaultException {
  const InvalidMasterPasswordException()
    : super('The master password is incorrect or the credential is damaged.');
}

class _WindowsCredentialVault {
  _WindowsCredentialVault() {
    if (!Platform.isWindows) {
      throw const CredentialVaultException(
        'Secure credential storage is currently available on Windows only.',
      );
    }
    final advapi = DynamicLibrary.open('advapi32.dll');
    final crypt32 = DynamicLibrary.open('crypt32.dll');
    final kernel32 = DynamicLibrary.open('kernel32.dll');
    _credWrite = advapi.lookupFunction<_CredWriteNative, _CredWriteDart>(
      'CredWriteW',
    );
    _credRead = advapi.lookupFunction<_CredReadNative, _CredReadDart>(
      'CredReadW',
    );
    _credDelete = advapi.lookupFunction<_CredDeleteNative, _CredDeleteDart>(
      'CredDeleteW',
    );
    _credFree = advapi.lookupFunction<_CredFreeNative, _CredFreeDart>(
      'CredFree',
    );
    _protect = crypt32.lookupFunction<_CryptProtectNative, _CryptProtectDart>(
      'CryptProtectData',
    );
    _unprotect = crypt32
        .lookupFunction<_CryptUnprotectNative, _CryptUnprotectDart>(
          'CryptUnprotectData',
        );
    _getLastError = kernel32
        .lookupFunction<_GetLastErrorNative, _GetLastErrorDart>('GetLastError');
    _localFree = kernel32.lookupFunction<_LocalFreeNative, _LocalFreeDart>(
      'LocalFree',
    );
  }

  late final _CredWriteDart _credWrite;
  late final _CredReadDart _credRead;
  late final _CredDeleteDart _credDelete;
  late final _CredFreeDart _credFree;
  late final _CryptProtectDart _protect;
  late final _CryptUnprotectDart _unprotect;
  late final _GetLastErrorDart _getLastError;
  late final _LocalFreeDart _localFree;

  void store({
    required String target,
    required String username,
    required String secret,
    required String? masterPassword,
  }) {
    final protectedByMaster = masterPassword?.isNotEmpty ?? false;
    final salt = protectedByMaster ? _randomBytes(16) : Uint8List(0);
    final entropy = protectedByMaster
        ? _pbkdf2Sha256(masterPassword!, salt, _pbkdf2Iterations, 32)
        : null;
    final plaintext = Uint8List.fromList(utf8.encode(secret));
    Uint8List? protectedData;
    try {
      var protection = 'dpapi';
      try {
        protectedData = _protectBytes(plaintext, entropy);
      } on CredentialVaultException {
        if (protectedByMaster) rethrow;
        // Credential Manager encrypts generic credentials with the signed-in
        // Windows account. Some managed or impersonated processes cannot call
        // DPAPI directly even though CredWrite is available, so let the vault
        // provide the at-rest encryption instead of blocking the connection.
        protectedData = Uint8List.fromList(plaintext);
        protection = 'windowsCredentialManager';
      }
      final envelope = utf8.encode(
        jsonEncode({
          'version': 2,
          'protection': protection,
          'masterPasswordProtected': protectedByMaster,
          'iterations': protectedByMaster ? _pbkdf2Iterations : 0,
          'salt': base64Encode(salt),
          'protectedData': base64Encode(protectedData),
        }),
      );
      if (envelope.length > 2400) {
        throw const CredentialVaultException(
          'The credential is too large for Windows Credential Manager.',
        );
      }
      _writeCredential(target, username, Uint8List.fromList(envelope));
    } finally {
      _wipe(plaintext);
      _wipe(entropy);
      _wipe(salt);
      _wipe(protectedData);
    }
  }

  CredentialMetadata? inspect(String target) {
    final raw = _readCredential(target);
    if (raw == null) return null;
    try {
      final envelope = _decodeEnvelope(raw.blob);
      return CredentialMetadata(
        username: raw.username,
        masterPasswordProtected:
            envelope['masterPasswordProtected'] as bool? ?? false,
      );
    } finally {
      _wipe(raw.blob);
    }
  }

  StoredCredential? read(String target, {String? masterPassword}) {
    final raw = _readCredential(target);
    if (raw == null) return null;
    Uint8List? salt;
    Uint8List? entropy;
    Uint8List? encrypted;
    Uint8List? plaintext;
    try {
      final envelope = _decodeEnvelope(raw.blob);
      final protectedByMaster =
          envelope['masterPasswordProtected'] as bool? ?? false;
      if (protectedByMaster &&
          (masterPassword == null || masterPassword.isEmpty)) {
        throw const MasterPasswordRequiredException();
      }
      salt = Uint8List.fromList(
        base64Decode(envelope['salt'] as String? ?? ''),
      );
      final iterations = (envelope['iterations'] as num?)?.toInt() ?? 0;
      if (protectedByMaster && iterations < 100000) {
        throw const CredentialVaultException(
          'The stored credential uses an unsupported protection format.',
        );
      }
      entropy = protectedByMaster
          ? _pbkdf2Sha256(masterPassword!, salt, iterations, 32)
          : null;
      encrypted = Uint8List.fromList(
        base64Decode(envelope['protectedData'] as String),
      );
      final protection = envelope['protection'] as String? ?? 'dpapi';
      if (protection == 'windowsCredentialManager') {
        if (protectedByMaster) {
          throw const CredentialVaultException(
            'The stored credential has an invalid protection policy.',
          );
        }
        plaintext = Uint8List.fromList(encrypted);
      } else if (protection == 'dpapi') {
        try {
          plaintext = _unprotectBytes(encrypted, entropy);
        } on CredentialVaultException {
          if (protectedByMaster) throw const InvalidMasterPasswordException();
          rethrow;
        }
      } else {
        throw const CredentialVaultException(
          'The stored credential uses an unknown protection method.',
        );
      }
      return StoredCredential(
        username: raw.username,
        secret: utf8.decode(plaintext, allowMalformed: false),
      );
    } on FormatException catch (error) {
      throw CredentialVaultException('Stored credential is damaged: $error');
    } finally {
      _wipe(raw.blob);
      _wipe(salt);
      _wipe(entropy);
      _wipe(encrypted);
      _wipe(plaintext);
    }
  }

  void updateUsername(String target, String username) {
    final raw = _readCredential(target);
    if (raw == null) {
      throw const CredentialVaultException(
        'The stored credential was not found.',
      );
    }
    try {
      _writeCredential(target, username, raw.blob);
    } finally {
      _wipe(raw.blob);
    }
  }

  void storeTemporaryRemoteDesktopCredential({
    required String targetHost,
    required String username,
    required String password,
  }) {
    final passwordBytes = _utf16LittleEndian(password);
    try {
      _writeCredential(
        'TERMSRV/$targetHost',
        username,
        passwordBytes,
        type: _credentialTypeDomainPassword,
        persist: _credentialPersistSession,
      );
    } finally {
      _wipe(passwordBytes);
    }
  }

  void deleteTemporaryRemoteDesktopCredential(String targetHost) {
    _deleteCredential(
      'TERMSRV/$targetHost',
      type: _credentialTypeDomainPassword,
    );
  }

  void delete(String target) {
    _deleteCredential(target);
  }

  void _deleteCredential(String target, {int type = _credentialTypeGeneric}) {
    final targetPointer = target.toNativeUtf16();
    try {
      if (_credDelete(targetPointer, type, 0) == 0) {
        final error = _getLastError();
        if (error != _errorNotFound) {
          throw CredentialVaultException(
            'Windows could not delete the credential (error $error).',
          );
        }
      }
    } finally {
      calloc.free(targetPointer);
    }
  }

  void _writeCredential(
    String target,
    String username,
    Uint8List blob, {
    int type = _credentialTypeGeneric,
    int persist = _credentialPersistLocalMachine,
  }) {
    final credential = calloc<_CredentialW>();
    final targetPointer = target.toNativeUtf16();
    final usernamePointer = username.toNativeUtf16();
    final blobPointer = calloc<Uint8>(blob.length);
    blobPointer.asTypedList(blob.length).setAll(0, blob);
    try {
      credential.ref
        ..flags = 0
        ..type = type
        ..targetName = targetPointer
        ..comment = nullptr
        ..credentialBlobSize = blob.length
        ..credentialBlob = blobPointer
        ..persist = persist
        ..attributeCount = 0
        ..attributes = nullptr
        ..targetAlias = nullptr
        ..userName = usernamePointer;
      if (_credWrite(credential, 0) == 0) {
        final error = _getLastError();
        throw CredentialVaultException(
          'Windows could not store the credential (error $error).',
        );
      }
    } finally {
      blobPointer.asTypedList(blob.length).fillRange(0, blob.length, 0);
      calloc.free(blobPointer);
      calloc.free(usernamePointer);
      calloc.free(targetPointer);
      calloc.free(credential);
    }
  }

  _RawCredential? _readCredential(String target) {
    final targetPointer = target.toNativeUtf16();
    final resultPointer = calloc<Pointer<_CredentialW>>();
    try {
      if (_credRead(targetPointer, _credentialTypeGeneric, 0, resultPointer) ==
          0) {
        final error = _getLastError();
        if (error == _errorNotFound) return null;
        throw CredentialVaultException(
          'Windows could not read the credential (error $error).',
        );
      }
      final credential = resultPointer.value;
      final size = credential.ref.credentialBlobSize;
      final blob = Uint8List.fromList(
        credential.ref.credentialBlob.asTypedList(size),
      );
      final username = credential.ref.userName == nullptr
          ? ''
          : credential.ref.userName.toDartString();
      return _RawCredential(username, blob);
    } finally {
      if (resultPointer.value != nullptr) {
        _credFree(resultPointer.value.cast<Void>());
      }
      calloc.free(resultPointer);
      calloc.free(targetPointer);
    }
  }

  Map<String, Object?> _decodeEnvelope(Uint8List blob) {
    final decoded = jsonDecode(utf8.decode(blob));
    if (decoded is! Map ||
        (decoded['version'] != 1 && decoded['version'] != 2)) {
      throw const CredentialVaultException(
        'The stored credential uses an unsupported format.',
      );
    }
    return Map<String, Object?>.from(decoded);
  }

  Uint8List _protectBytes(Uint8List plaintext, Uint8List? entropy) {
    return _runDpapi(plaintext, entropy, (input, entropyPointer, output) {
      final description = 'AetherDesk credential'.toNativeUtf16();
      try {
        return _protect(
          input,
          description,
          entropyPointer,
          nullptr,
          nullptr,
          _cryptProtectUiForbidden,
          output,
        );
      } finally {
        calloc.free(description);
      }
    }, 'encrypt');
  }

  Uint8List _unprotectBytes(Uint8List encrypted, Uint8List? entropy) {
    return _runDpapi(
      encrypted,
      entropy,
      (input, entropyPointer, output) => _unprotect(
        input,
        nullptr,
        entropyPointer,
        nullptr,
        nullptr,
        _cryptProtectUiForbidden,
        output,
      ),
      'decrypt',
    );
  }

  Uint8List _runDpapi(
    Uint8List inputBytes,
    Uint8List? entropyBytes,
    int Function(Pointer<_DataBlob>, Pointer<_DataBlob>, Pointer<_DataBlob>)
    operation,
    String action,
  ) {
    final input = calloc<_DataBlob>();
    final inputData = calloc<Uint8>(inputBytes.length);
    final output = calloc<_DataBlob>();
    Pointer<_DataBlob>? entropy;
    Pointer<Uint8>? entropyData;
    inputData.asTypedList(inputBytes.length).setAll(0, inputBytes);
    input.ref
      ..length = inputBytes.length
      ..data = inputData;
    if (entropyBytes != null && entropyBytes.isNotEmpty) {
      entropy = calloc<_DataBlob>();
      entropyData = calloc<Uint8>(entropyBytes.length);
      entropyData.asTypedList(entropyBytes.length).setAll(0, entropyBytes);
      entropy.ref
        ..length = entropyBytes.length
        ..data = entropyData;
    }
    try {
      final entropyPointer = entropy ?? nullptr.cast<_DataBlob>();
      if (operation(input, entropyPointer, output) == 0) {
        final error = _getLastError();
        throw CredentialVaultException(
          'Windows could not $action the credential (error $error).',
        );
      }
      return Uint8List.fromList(output.ref.data.asTypedList(output.ref.length));
    } finally {
      inputData
          .asTypedList(inputBytes.length)
          .fillRange(0, inputBytes.length, 0);
      if (entropyData != null && entropyBytes != null) {
        entropyData
            .asTypedList(entropyBytes.length)
            .fillRange(0, entropyBytes.length, 0);
      }
      if (output.ref.data != nullptr) {
        output.ref.data
            .asTypedList(output.ref.length)
            .fillRange(0, output.ref.length, 0);
        _localFree(output.ref.data.cast<Void>());
      }
      if (entropyData != null) calloc.free(entropyData);
      if (entropy != null) calloc.free(entropy);
      calloc.free(output);
      calloc.free(inputData);
      calloc.free(input);
    }
  }
}

class _RawCredential {
  const _RawCredential(this.username, this.blob);

  final String username;
  final Uint8List blob;
}

Uint8List _randomBytes(int count) {
  final random = Random.secure();
  return Uint8List.fromList(
    List<int>.generate(count, (_) => random.nextInt(256)),
  );
}

Uint8List _pbkdf2Sha256(
  String password,
  Uint8List salt,
  int iterations,
  int length,
) {
  final passwordBytes = Uint8List.fromList(utf8.encode(password));
  final hmac = Hmac(sha256, passwordBytes);
  final output = Uint8List(length);
  try {
    var outputOffset = 0;
    for (var block = 1; outputOffset < length; block++) {
      final initial = BytesBuilder(copy: false)
        ..add(salt)
        ..add([
          (block >> 24) & 0xff,
          (block >> 16) & 0xff,
          (block >> 8) & 0xff,
          block & 0xff,
        ]);
      var value = Uint8List.fromList(hmac.convert(initial.takeBytes()).bytes);
      final combined = Uint8List.fromList(value);
      for (var round = 1; round < iterations; round++) {
        final next = Uint8List.fromList(hmac.convert(value).bytes);
        _wipe(value);
        value = next;
        for (var index = 0; index < combined.length; index++) {
          combined[index] ^= value[index];
        }
      }
      final take = min(combined.length, length - outputOffset);
      output.setRange(outputOffset, outputOffset + take, combined);
      outputOffset += take;
      _wipe(value);
      _wipe(combined);
    }
    return output;
  } finally {
    _wipe(passwordBytes);
  }
}

void _wipe(Uint8List? bytes) {
  if (bytes != null) bytes.fillRange(0, bytes.length, 0);
}

Uint8List _utf16LittleEndian(String value) {
  final data = ByteData(value.codeUnits.length * 2);
  for (var index = 0; index < value.codeUnits.length; index++) {
    data.setUint16(index * 2, value.codeUnitAt(index), Endian.little);
  }
  return data.buffer.asUint8List();
}
