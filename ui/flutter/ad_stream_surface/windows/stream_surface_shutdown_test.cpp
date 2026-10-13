// Reproduce Flutter's native plugin destruction order without starting an engine.
// Compile the production plugin and real Flutter client wrapper against these
// engine C API doubles. A callback after messenger detachment is always a failure.
#define NOMINMAX
#include <flutter/plugin_registrar_windows.h>
#include <flutter/standard_method_codec.h>
#include <flutter_windows.h>
#include <cstdlib>
#include <iostream>
#include <map>
#include <string>
#include <vector>
#ifdef NDEBUG
#undef NDEBUG
#endif
#include <cassert>
#include "include/ad_stream_surface/ad_stream_surface_plugin_c_api.h"

struct FlutterDesktopMessenger {
  bool available = true;
  int callback_sets = 0;
  int callback_clears = 0;
  int references = 0;
  int responses = 0;
  std::map<std::string, std::pair<FlutterDesktopMessageCallback, void*>> handlers;
};
struct FlutterDesktopTextureRegistrar {};
struct FlutterDesktopPluginRegistrar {
  FlutterDesktopMessenger messenger;
  FlutterDesktopTextureRegistrar textures;
  FlutterDesktopOnPluginRegistrarDestroyed on_destroy = nullptr;
};

FlutterDesktopMessengerRef FlutterDesktopPluginRegistrarGetMessenger(
    FlutterDesktopPluginRegistrarRef registrar) {
  return &registrar->messenger;
}
void FlutterDesktopPluginRegistrarSetDestructionHandler(
    FlutterDesktopPluginRegistrarRef registrar,
    FlutterDesktopOnPluginRegistrarDestroyed callback) {
  registrar->on_destroy = callback;
}
FlutterDesktopViewRef FlutterDesktopPluginRegistrarGetView(
    FlutterDesktopPluginRegistrarRef) { return nullptr; }
HWND FlutterDesktopViewGetHWND(FlutterDesktopViewRef) { return nullptr; }
FlutterDesktopTextureRegistrarRef FlutterDesktopRegistrarGetTextureRegistrar(
    FlutterDesktopPluginRegistrarRef registrar) { return &registrar->textures; }

bool FlutterDesktopMessengerSend(FlutterDesktopMessengerRef, const char*,
                                 const uint8_t*, size_t) { return true; }
bool FlutterDesktopMessengerSendWithReply(FlutterDesktopMessengerRef,
    const char*, const uint8_t*, size_t, FlutterDesktopBinaryReply, void*) {
  return true;
}
void FlutterDesktopMessengerSendResponse(FlutterDesktopMessengerRef messenger,
    const FlutterDesktopMessageResponseHandle*, const uint8_t*, size_t) {
  assert(messenger->available);
  ++messenger->responses;
}
void FlutterDesktopMessengerSetCallback(FlutterDesktopMessengerRef messenger,
    const char* channel, FlutterDesktopMessageCallback callback, void* user_data) {
  // The real engine dereferences a null engine here after native child shutdown.
  if (!messenger->available) {
    std::cerr << "Callback registration after engine detachment\n";
    std::exit(1);
  }
  if (callback) {
    ++messenger->callback_sets;
    messenger->handlers[channel] = {callback, user_data};
  } else {
    ++messenger->callback_clears;
    messenger->handlers.erase(channel);
  }
}
FlutterDesktopMessengerRef FlutterDesktopMessengerAddRef(
    FlutterDesktopMessengerRef messenger) { ++messenger->references; return messenger; }
void FlutterDesktopMessengerRelease(FlutterDesktopMessengerRef messenger) {
  --messenger->references;
  assert(messenger->references >= 0);
}
bool FlutterDesktopMessengerIsAvailable(FlutterDesktopMessengerRef messenger) {
  return messenger->available;
}
FlutterDesktopMessengerRef FlutterDesktopMessengerLock(
    FlutterDesktopMessengerRef messenger) { return messenger; }
void FlutterDesktopMessengerUnlock(FlutterDesktopMessengerRef) {}
int64_t FlutterDesktopTextureRegistrarRegisterExternalTexture(
    FlutterDesktopTextureRegistrarRef, const FlutterDesktopTextureInfo*) { return 1; }
void FlutterDesktopTextureRegistrarUnregisterExternalTexture(
    FlutterDesktopTextureRegistrarRef, int64_t, void (*callback)(void*), void* data) {
  if (callback) callback(data);
}
bool FlutterDesktopTextureRegistrarMarkExternalTextureFrameAvailable(
    FlutterDesktopTextureRegistrarRef, int64_t) { return true; }

namespace {
constexpr char kChannel[] = "aetherdesk/stream_surface";
void CheckMainStillResponds(FlutterDesktopPluginRegistrar& main) {
  auto message = flutter::StandardMethodCodec::GetInstance().EncodeMethodCall(
      flutter::MethodCall<flutter::EncodableValue>("isPresented",
          std::make_unique<flutter::EncodableValue>(int64_t{42})));
  FlutterDesktopMessage request{};
  request.struct_size = sizeof(request);
  request.channel = kChannel;
  request.message = message->data();
  request.message_size = message->size();
  request.response_handle = reinterpret_cast<FlutterDesktopMessageResponseHandle*>(1);
  const int before = main.messenger.responses;
  auto handler = main.messenger.handlers.at(kChannel);
  handler.first(&main.messenger, &request, handler.second);
  assert(main.messenger.responses == before + 1);
  assert(main.messenger.references == 0);
}
void Destroy(FlutterDesktopPluginRegistrar& registrar, bool detach_engine) {
  assert(registrar.on_destroy);
  registrar.messenger.available = !detach_engine;
  registrar.on_destroy(&registrar);
  // The engine discards the remaining callbacks after plugin destruction.
  registrar.messenger.handlers.clear();
}
}  // namespace

int main() {
  FlutterDesktopPluginRegistrar main;
  AdStreamSurfacePluginCApiRegisterWithRegistrar(&main);
  CheckMainStillResponds(main);
  for (int i = 0; i < 100; ++i) {
    FlutterDesktopPluginRegistrar editor;
    AdStreamSurfacePluginCApiRegisterWithRegistrar(&editor);
    assert(editor.messenger.callback_sets == 1);
    // This also covers an editor closed immediately, without any stream/WebView.
    Destroy(editor, true);
    assert(editor.messenger.callback_clears == 0);
    assert(main.messenger.callback_clears == 0);
    CheckMainStillResponds(main);
  }
  // Normal live-engine plugin removal must still unregister the stale callback.
  FlutterDesktopPluginRegistrar live;
  AdStreamSurfacePluginCApiRegisterWithRegistrar(&live);
  Destroy(live, false);
  assert(live.messenger.callback_clears == 1);
  Destroy(main, true);
  assert(main.messenger.callback_clears == 0);
  std::cout << "Native editor teardown regression passed (100 child cycles).\n";
}
