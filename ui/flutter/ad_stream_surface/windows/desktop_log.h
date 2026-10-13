#pragma once
// Tiny event logger for the native presentation layer (log_feature.md §3.2 D).
// One JSONL record per event in logs/native-PID.log under LOCALAPPDATA/AetherDesk.
// Separate rotation ownership lets the Logs panel merge it safely with the
// Rust core and Dart records. Events only — never call this per frame.

#if defined(_WIN32)
#include <windows.h>

#include <cstdio>
#include <mutex>
#include <string>

inline void AdNativeLog(const char* level, const char* message) {
  static std::mutex mutex;
  static std::wstring path;
  static bool resolved = false;
  std::lock_guard<std::mutex> lock(mutex);
  if (!resolved) {
    resolved = true;
    wchar_t base[32768] = {};
    const DWORD length = GetEnvironmentVariableW(L"LOCALAPPDATA", base, 32768);
    if (length > 0 && length < 32768) {
      const std::wstring root = std::wstring(base) + L"\\AetherDesk";
      CreateDirectoryW(root.c_str(), nullptr);
      CreateDirectoryW((root + L"\\logs").c_str(), nullptr);
      path = root + L"\\logs\\native-" + std::to_wstring(GetCurrentProcessId()) + L".log";
    }
  }
  if (path.empty()) return;
  std::string escaped;
  escaped.reserve(128);
  for (const char* p = message; *p; ++p) {
    if (*p == '"' || *p == '\\') escaped.push_back('\\');
    if (static_cast<unsigned char>(*p) < 0x20) continue;
    escaped.push_back(*p);
  }
  SYSTEMTIME time;
  GetSystemTime(&time);
  WIN32_FILE_ATTRIBUTE_DATA attributes{};
  if (GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &attributes) &&
      (attributes.nFileSizeHigh || attributes.nFileSizeLow >= 10 * 1024 * 1024)) {
    DeleteFileW((path + L".5").c_str());
    for (int i = 4; i >= 1; --i)
      MoveFileExW((path + L"." + std::to_wstring(i)).c_str(), (path + L"." + std::to_wstring(i+1)).c_str(), MOVEFILE_REPLACE_EXISTING);
    MoveFileExW(path.c_str(), (path + L".1").c_str(), MOVEFILE_REPLACE_EXISTING);
  }
  FILE* file = nullptr;
  if (_wfopen_s(&file, path.c_str(), L"a") != 0 || !file) return;
  fprintf(file,
          "{\"ts\":\"%04u-%02u-%02uT%02u:%02u:%02u.%03uZ\",\"level\":\"%s\","
          "\"source\":\"ui.native\",\"msg\":\"%s\"}\n",
          time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute,
          time.wSecond, time.wMilliseconds, level, escaped.c_str());
  fclose(file);
}
#else
inline void AdNativeLog(const char*, const char*) {}
#endif
