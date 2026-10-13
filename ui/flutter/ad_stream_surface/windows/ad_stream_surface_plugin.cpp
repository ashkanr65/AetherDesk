#ifndef NOMINMAX
#define NOMINMAX
#endif
#include "include/ad_stream_surface/ad_stream_surface_plugin_c_api.h"

#include <flutter/method_channel.h>
#include <flutter/plugin_registrar_windows.h>
#include <flutter/standard_method_codec.h>
#include <flutter/texture_registrar.h>
#include <windows.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>
#include <set>
#include <sstream>
#include <windowsx.h>
#include "desktop_platform.h"

namespace {
struct RemoteFrame {
  uint32_t width;
  uint32_t height;
  uint64_t sequence;
  uint8_t* data;
  size_t length;
};
using AcquireFrame = RemoteFrame* (*)(const char*, uint32_t);
using ReleaseFrame = void (*)(RemoteFrame*);

struct Pixels {
  size_t width;
  size_t height;
  uint64_t sequence = 0;
  std::vector<uint8_t> rgba;
  std::vector<uint8_t> bgra;
};

// Each raster callback owns its immutable snapshot until Flutter releases it.
struct PixelView {
  FlutterDesktopPixelBuffer buffer{};
  std::shared_ptr<const Pixels> pixels;
  explicit PixelView(std::shared_ptr<const Pixels> value) : pixels(std::move(value)) {
    buffer.buffer = pixels->rgba.data();
    buffer.width = pixels->width;
    buffer.height = pixels->height;
    buffer.release_context = this;
    buffer.release_callback = [](void* context) { delete static_cast<PixelView*>(context); };
  }
};

struct Feed {
  flutter::TextureRegistrar* registrar;
  std::string session;
  AcquireFrame acquire;
  ReleaseFrame release;
  int64_t texture_id = -1;
  std::atomic<bool> running{false};
  std::atomic<bool> presenting{false};
  std::thread worker;
  std::mutex mutex;
  std::shared_ptr<const Pixels> latest;

  Feed(flutter::TextureRegistrar* r, std::string s, AcquireFrame a, ReleaseFrame f)
      : registrar(r), session(std::move(s)), acquire(a), release(f) {}
  ~Feed() { Stop(); }
  void Stop() {
    running.store(false);
    if (worker.joinable()) worker.join();
  }
  const FlutterDesktopPixelBuffer* Read() {
    std::shared_ptr<const Pixels> pixels;
    { std::lock_guard<std::mutex> lock(mutex); pixels = latest; }
    if (!pixels) return nullptr;
    return &(new PixelView(std::move(pixels)))->buffer;
  }
  void Start() {
    running.store(true);
    worker = std::thread([this]() {
      while (running.load()) {
        RemoteFrame* frame = acquire(session.c_str(), 100);
        if (frame) {
          const size_t width = frame->width;
          const size_t height = frame->height;
          if (width > 0 && height > 0 && width <= 16384 && height <= 16384 && width*height <= 33554432 &&
              frame->data && frame->length == width * height * 4) {
            auto pixels = std::make_shared<Pixels>();
            pixels->width = width;
            pixels->height = height;
            pixels->sequence = frame->sequence;
            if (presenting.load()) pixels->bgra.assign(frame->data, frame->data + frame->length);
            pixels->rgba.resize(frame->length);
            for (size_t i = 0; i < frame->length; i += 4) {
              pixels->rgba[i] = frame->data[i + 2];
              pixels->rgba[i + 1] = frame->data[i + 1];
              pixels->rgba[i + 2] = frame->data[i];
              pixels->rgba[i + 3] = frame->data[i + 3];
            }
            { std::lock_guard<std::mutex> lock(mutex); latest = std::move(pixels); }
            registrar->MarkTextureFrameAvailable(texture_id);
          }
          release(frame);
        } else {
          // A removed session returns immediately instead of waiting for a frame.
          std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
      }
    });
  }
};

#include "desktop_windows.h"

struct Surface {
  std::shared_ptr<Feed> feed;
  std::unique_ptr<flutter::TextureVariant> texture;
  std::unique_ptr<DesktopWindows> windows;
};

class StreamSurfacePlugin : public flutter::Plugin {
 public:
  StreamSurfacePlugin(flutter::PluginRegistrarWindows* registrar,
                      FlutterDesktopMessengerRef messenger)
      : registrar_(registrar), messenger_(messenger) {
    channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
        registrar->messenger(), "aetherdesk/stream_surface", &flutter::StandardMethodCodec::GetInstance());
    channel_->SetMethodCallHandler([this](const auto& call, auto result) { Handle(call, std::move(result)); });
  }
  ~StreamSurfacePlugin() override {
    // Flutter detaches the messenger from its engine before destroying plugins.
    // SetMethodCallHandler still dereferences that engine, even when clearing
    // a handler. The registrar owns this messenger until our destructor ends.
    if (FlutterDesktopMessengerIsAvailable(messenger_)) {
      channel_->SetMethodCallHandler(nullptr);
    }
    for (auto& entry : surfaces_) {
      auto surface = entry.second;
      surface->windows.reset();
      surface->feed->Stop();
      registrar_->texture_registrar()->UnregisterTexture(entry.first, [surface]() {});
    }
  }

 private:
  void Handle(const flutter::MethodCall<flutter::EncodableValue>& call,
              std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
    if (call.method_name() == "keyboardLayout") {
      const uint32_t id = CurrentKeyboardId();
      const auto mapping = ad_desktop::KeyboardForId(id);
      if (!mapping.layout) { result->Success(); return; }
      result->Success(flutter::EncodableValue(flutter::EncodableMap{
        {flutter::EncodableValue("layout"), flutter::EncodableValue(mapping.layout)},
        {flutter::EncodableValue("variant"), flutter::EncodableValue(mapping.variant)},
      }));
    } else if (call.method_name() == "present" || call.method_name() == "setTabSize") {
      const auto* args = call.arguments() ? std::get_if<flutter::EncodableMap>(call.arguments()) : nullptr;
      if (!args) { result->Error("invalid_view", "Desktop view is required"); return; }
      const auto id_value = args->find(flutter::EncodableValue("textureId"));
      const auto all_value = args->find(flutter::EncodableValue("allMonitors"));
      int64_t id = -1;
      if (id_value != args->end()) {
        if (const auto* wide_value = std::get_if<int64_t>(&id_value->second)) id = *wide_value;
        else if (const auto* narrow_value = std::get_if<int32_t>(&id_value->second)) id = *narrow_value;
      }
      const auto found = surfaces_.find(id);
      if (found == surfaces_.end()) { result->Error("closed_view", "Desktop view is closed"); return; }
      const bool* all = all_value == args->end() ? nullptr : std::get_if<bool>(&all_value->second);
      const auto core = GetModuleHandleW(L"ad_ffi.dll");
      const auto command = core ? reinterpret_cast<RemoteCommand>(GetProcAddress(core, "ad_remote_command")) : nullptr;
      auto& surface = found->second;
      if (!surface->windows) surface->windows = std::make_unique<DesktopWindows>(surface->feed, registrar_->GetView()->GetNativeWindow(), command);
      if (call.method_name() == "setTabSize") {
        auto dimension = [&](const char* name) -> int64_t {
          const auto value = args->find(flutter::EncodableValue(name));
          if (value == args->end()) return 0;
          if (const auto* wide = std::get_if<int64_t>(&value->second)) return *wide;
          if (const auto* narrow = std::get_if<int32_t>(&value->second)) return *narrow;
          return 0;
        };
        const auto width = dimension("width"), height = dimension("height");
        if (width < 64 || height < 64 || width > 16384 || height > 16384 || width * height > 33554432) {
          result->Error("invalid_size", "Remote tab size is invalid"); return;
        }
        const auto force_value = args->find(flutter::EncodableValue("force"));
        const bool* force = force_value == args->end() ? nullptr : std::get_if<bool>(&force_value->second);
        if (!surface->windows->SetTabSize(static_cast<uint32_t>(width), static_cast<uint32_t>(height), force && *force)) {
          result->Error("resize_error", "Could not update the remote desktop size"); return;
        }
        result->Success(); return;
      }
      if (!surface->windows->Open(all && *all)) {
        AdNativeLog("error", "Could not open the desktop on the selected monitors");
        surface->windows.reset();
        result->Error("display_error", "Could not open the desktop on the selected monitors");
        return;
      }
      result->Success();
    } else if (call.method_name() == "create") {
      const auto* session = call.arguments() ? std::get_if<std::string>(call.arguments()) : nullptr;
      if (!session || session->size() != 36) { result->Error("invalid_session", "Remote session ID is required"); return; }
      // Dart has already loaded the packaged core. Never search PATH for another DLL.
      HMODULE core = GetModuleHandleW(L"ad_ffi.dll");
      const auto acquire = core ? reinterpret_cast<AcquireFrame>(GetProcAddress(core, "ad_remote_wait_frame")) : nullptr;
      const auto release = core ? reinterpret_cast<ReleaseFrame>(GetProcAddress(core, "ad_remote_release_frame")) : nullptr;
      if (!acquire || !release) { result->Error("missing_core", "The desktop-enabled AetherDesk core is not loaded"); return; }
      auto surface = std::make_shared<Surface>();
      auto feed = std::make_shared<Feed>(registrar_->texture_registrar(), *session, acquire, release);
      surface->feed = feed;
      surface->texture = std::make_unique<flutter::TextureVariant>(flutter::PixelBufferTexture(
          [feed](size_t, size_t) { return feed->Read(); }));
      const int64_t id = registrar_->texture_registrar()->RegisterTexture(surface->texture.get());
      if (id < 0) {
        AdNativeLog("error", "Flutter rejected the desktop texture");
        result->Error("texture_error", "Flutter rejected the desktop texture");
        return;
      }
      feed->texture_id = id;
      surfaces_.emplace(id, surface);
      feed->Start();
      result->Success(flutter::EncodableValue(id));
    } else if (call.method_name() == "dispose" || call.method_name() == "isPresented" || call.method_name() == "capture") {
      int64_t id = -1;
      if (call.arguments()) {
        if (const auto* wide_value = std::get_if<int64_t>(call.arguments())) id = *wide_value;
        else if (const auto* narrow_value = std::get_if<int32_t>(call.arguments())) id = *narrow_value;
      }
      const auto found = surfaces_.find(id);
      if (call.method_name() == "capture") {
        if (found == surfaces_.end()) { result->Error("closed", "The remote screen is closed."); return; }
        std::shared_ptr<const Pixels> pixels;
        { std::lock_guard<std::mutex> lock(found->second->feed->mutex); pixels = found->second->feed->latest; }
        if (!pixels) { result->Error("no_frame", "The remote screen has not arrived yet."); return; }
        // Bound the method-channel copy even for a large multi-monitor desktop.
        const double scale = std::min(1.0, 1600.0 / static_cast<double>(std::max(pixels->width, pixels->height)));
        const size_t width = std::max<size_t>(1, static_cast<size_t>(pixels->width * scale));
        const size_t height = std::max<size_t>(1, static_cast<size_t>(pixels->height * scale));
        std::vector<uint8_t> rgba(width * height * 4);
        for (size_t y = 0; y < height; ++y) for (size_t x = 0; x < width; ++x) {
          const size_t source = ((y * pixels->height / height) * pixels->width + (x * pixels->width / width)) * 4;
          std::copy_n(pixels->rgba.data() + source, 4, rgba.data() + (y * width + x) * 4);
        }
        result->Success(flutter::EncodableValue(flutter::EncodableMap{
          {flutter::EncodableValue("width"), flutter::EncodableValue(static_cast<int32_t>(width))},
          {flutter::EncodableValue("height"), flutter::EncodableValue(static_cast<int32_t>(height))},
          {flutter::EncodableValue("rgba"), flutter::EncodableValue(std::move(rgba))},
        }));
        return;
      }
      if (call.method_name() == "isPresented") {
        result->Success(flutter::EncodableValue(found != surfaces_.end() && found->second->windows && found->second->windows->HasFocus())); return;
      }
      if (found != surfaces_.end()) {
        auto surface = found->second;
        surface->windows.reset();
        surface->feed->Stop();
        surfaces_.erase(found);
        registrar_->texture_registrar()->UnregisterTexture(id, [surface]() {});
      }
      result->Success();
    } else { result->NotImplemented(); }
  }
  flutter::PluginRegistrarWindows* registrar_;
  FlutterDesktopMessengerRef messenger_;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  std::map<int64_t, std::shared_ptr<Surface>> surfaces_;
};
}  // namespace

void AdStreamSurfacePluginCApiRegisterWithRegistrar(FlutterDesktopPluginRegistrarRef registrar) {
  auto* windows_registrar = flutter::PluginRegistrarManager::GetInstance()
      ->GetRegistrar<flutter::PluginRegistrarWindows>(registrar);
  windows_registrar->AddPlugin(std::make_unique<StreamSurfacePlugin>(
      windows_registrar, FlutterDesktopPluginRegistrarGetMessenger(registrar)));
}
