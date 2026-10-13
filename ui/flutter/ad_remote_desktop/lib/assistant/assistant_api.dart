import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// Authenticated loopback transport. Redirects never receive our credentials.
class AssistantApi {
  AssistantApi(this.baseUri, this.password, {HttpClient? client})
    : _client = client ?? HttpClient() {
    if (baseUri.scheme != 'http' || baseUri.host != '127.0.0.1') {
      throw ArgumentError('AI Assistant must use the private loopback server.');
    }
    _client.connectionTimeout = const Duration(seconds: 5);
    _client.findProxy = (_) => 'DIRECT';
  }
  final Uri baseUri;
  final String password;
  final HttpClient _client;
  String? directory;

  Future<HttpClientRequest> _open(String method, String path) async {
    final uri = baseUri.replace(
      path: path,
      queryParameters: directory == null ? null : {'directory': directory!},
    );
    final request = await _client.openUrl(method, uri);
    request.followRedirects = false;
    request.headers.set(
      HttpHeaders.authorizationHeader,
      'Basic ${base64Encode(utf8.encode('aetherdesk:$password'))}',
    );
    return request;
  }

  Future<Object?> request(String method, String path, [Object? body]) async {
    final request = await _open(method, path);
    if (body != null) {
      final bytes = utf8.encode(jsonEncode(body));
      request.headers.contentType = ContentType.json;
      request.contentLength = bytes.length;
      request.add(bytes);
    }
    final response = await request.close().timeout(const Duration(seconds: 30));
    final data = await _readLimited(
      response,
      16 * 1024 * 1024,
    ).timeout(const Duration(seconds: 30));
    if (response.statusCode < 200 || response.statusCode >= 300) {
      // Never echo a request body or an upstream response that may contain keys.
      throw HttpException(
        'AI Assistant could not complete the request (HTTP ${response.statusCode}).',
      );
    }
    return data.isEmpty ? null : jsonDecode(data);
  }

  Stream<Map<String, dynamic>> events() async* {
    final request = await _open('GET', '/event');
    request.headers.set(HttpHeaders.acceptHeader, 'text/event-stream');
    final response = await request.close().timeout(const Duration(seconds: 15));
    if (response.statusCode != HttpStatus.ok) {
      await response.drain<void>();
      throw HttpException(
        'AI Assistant event connection failed (${response.statusCode}).',
      );
    }
    var data = <String>[];
    var size = 0;
    await for (final line
        in response.transform(utf8.decoder).transform(const LineSplitter())) {
      if (line.isEmpty) {
        if (data.isNotEmpty) {
          final value = jsonDecode(data.join('\n'));
          if (value is Map) yield Map<String, dynamic>.from(value);
        }
        data = [];
        size = 0;
      } else if (line.startsWith('data:')) {
        final value = line.substring(5).trimLeft();
        size += value.length;
        if (size > 4 * 1024 * 1024)
          throw const FormatException(
            'AI Assistant event exceeded the size limit.',
          );
        data.add(value);
      }
    }
  }

  static Future<String> _readLimited(
    Stream<List<int>> stream,
    int limit,
  ) async {
    final bytes = <int>[];
    await for (final chunk in stream) {
      if (bytes.length + chunk.length > limit)
        throw const FormatException(
          'AI Assistant response exceeded the size limit.',
        );
      bytes.addAll(chunk);
    }
    return utf8.decode(bytes);
  }

  void close() => _client.close(force: true);
}
