// Exercise native presentation without opening windows on the user's desktop.
#define NOMINMAX
#include <windows.h>
#include <windowsx.h>
#include <atomic>
#include <cstdint>
#include <memory>
#include <mutex>
#include <set>
#include <sstream>
#include <string>
#include <vector>
#include "desktop_platform.h"
#ifdef NDEBUG
#undef NDEBUG
#endif
#include <cassert>

namespace {
struct Pixels {
  size_t width = 4, height = 4;
  uint64_t sequence = 1;
  std::vector<uint8_t> bgra, rgba;
};
struct Feed {
  std::string session = "test";
  std::mutex mutex;
  std::shared_ptr<const Pixels> latest;
  std::atomic<bool> presenting{true};
};
#include "desktop_windows.h"

struct DesktopWindowsTest {
  inline static std::vector<std::string> commands;
  inline static bool fail_send = false;
  static int Command(const char*, const char* json) { if (fail_send) return -1; commands.emplace_back(json); return 0; }
  // Vertical docking describes the order of the controls, not the outline's
  // aspect ratio: readable labels make the side bar wider than its total height
  // at normal DPI (180 x 152). Verify the actual child-window layout instead.
  static void AssertVerticalToolbar(const DesktopWindows::View& view) {
    RECT client{};
    assert(GetClientRect(view.window, &client));
    const RECT& bar = view.toolbar_rect;
    assert(bar.left >= client.left && bar.right <= client.right);
    assert(bar.top >= client.top && bar.bottom <= client.bottom);
    assert(bar.right > bar.left && bar.bottom > bar.top);
    assert(view.handle_rect.left == bar.left && view.handle_rect.right == bar.right);
    assert(view.handle_rect.top == bar.top && view.handle_rect.bottom > bar.top);
    LONG next_top = view.handle_rect.bottom;
    for (HWND button : view.buttons) {
      RECT bounds{};
      assert(IsWindow(button) && GetWindowRect(button, &bounds));
      MapWindowPoints(nullptr, view.window, reinterpret_cast<POINT*>(&bounds), 2);
      assert(bounds.left == bar.left && bounds.right == bar.right);
      assert(bounds.top == next_top && bounds.bottom > bounds.top);
      assert(bounds.bottom <= bar.bottom);
      next_top = bounds.bottom;
    }
    assert(next_top == bar.bottom);
  }
  static DWORD Run() {
    const auto original_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    {
      PhysicalDpiScope scope;
      assert(AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2));
    }
    assert(AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_UNAWARE));
    if (original_dpi) SetThreadDpiAwarenessContext(original_dpi);
    WNDCLASSW klass{};
    klass.lpfnWndProc = DesktopWindows::WindowProc;
    klass.hInstance = GetModuleHandleW(nullptr);
    klass.lpszClassName = L"AetherDeskInvisiblePresentationTest";
    assert(RegisterClassW(&klass));
    {
      auto feed = std::make_shared<Feed>();
      auto pixels = std::make_shared<Pixels>();
      for (int i = 0; i < 16; ++i) pixels->bgra.insert(pixels->bgra.end(), {30, 180, 70, 255});
      feed->latest = pixels;
      DesktopWindows desktop(feed, nullptr, Command);
      // A static desktop has only the tab's RGBA snapshot before presentation.
      pixels->rgba = {70, 180, 30, 255}; pixels->bgra.clear();
      desktop.PreparePixels();
      assert((feed->latest->bgra == std::vector<uint8_t>{30, 180, 70, 255}));
      assert(pixels->bgra.empty()); // Existing texture consumers retain immutable data.
      for (int i = 0; i < 16; ++i) pixels->bgra.insert(pixels->bgra.end(), {30, 180, 70, 255});
      feed->latest = pixels;
      desktop.layout_ = ad_desktop::Arrange({{0, 0, 4, 4}, {4, 0, 4, 4}});
      // Create hidden views, with exactly the same window procedure as fullscreen.
      for (int i = 0; i < 2; ++i) {
        auto view = std::make_unique<DesktopWindows::View>();
        view->owner = &desktop; view->region = desktop.layout_.monitors[i]; view->menu = false;
        view->window = CreateWindowW(klass.lpszClassName, L"Test", WS_POPUP,
            0, 0, 32, 32, nullptr, nullptr, klass.hInstance, view.get());
        assert(view->window);
        desktop.views_.push_back(std::move(view));
      }
      auto& view = *desktop.views_.front();
      desktop.Paint(view);
      assert(GetPixel(view.buffer, 16, 16) == RGB(70, 180, 30));
      const HBITMAP first = view.bitmap;
      for (int i = 0; i < 100; ++i) desktop.Paint(view);
      assert(view.bitmap == first); // No allocation for steady-size frame updates.
      SetWindowPos(view.window, nullptr, 0, 0, 64, 48, SWP_NOZORDER | SWP_NOACTIVATE);
      desktop.Paint(view);
      assert(view.width == 64 && view.height == 48);
      assert(GetPixel(view.buffer, 63, 47) == RGB(70, 180, 30));
      view.toolbar = true;
      desktop.CreateToolbar(view);
      for (int width : {320, 1920, 3840}) {
        SetWindowPos(view.window, nullptr, -1920, -200, width, 1080, SWP_NOZORDER | SWP_NOACTIVATE);
        desktop.LayoutToolbar(view);
        assert(view.toolbar_rect.left >= 0 && view.toolbar_rect.right <= width);
        assert(abs(view.toolbar_rect.left + view.toolbar_rect.right - width) <= 1);
        for (HWND button : view.buttons) {
          assert(IsWindow(button));
          RECT bounds{}; GetWindowRect(button, &bounds);
          MapWindowPoints(nullptr, view.window, reinterpret_cast<POINT*>(&bounds), 2);
          assert(bounds.left >= 0 && bounds.right <= width && bounds.top == 0);
        }
      }
      assert(!desktop.views_[1]->toolbar);

      // The hamburger handle drags the bar; edges snap inside a margin.
      SetWindowPos(view.window, nullptr, 0, 0, 1200, 900, SWP_NOZORDER | SWP_NOACTIVATE);
      desktop.toolbar_edge_ = DesktopWindows::ToolbarEdge::Top;
      desktop.toolbar_floating_ = false;
      desktop.LayoutToolbar(view);
      assert(view.handle_rect.left == view.toolbar_rect.left);
      assert(view.handle_rect.right > view.handle_rect.left);
      const POINT grab{view.handle_rect.left + 4, view.handle_rect.top + 4};
      SendMessageW(view.window, WM_LBUTTONDOWN, MK_LBUTTON, MAKELPARAM(grab.x, grab.y));
      assert(desktop.toolbar_dragging_);
      SendMessageW(view.window, WM_MOUSEMOVE, MK_LBUTTON, MAKELPARAM(600, 880));
      assert(desktop.toolbar_edge_ == DesktopWindows::ToolbarEdge::Bottom);
      assert(!desktop.toolbar_floating_ && view.toolbar_rect.bottom == 900);
      SendMessageW(view.window, WM_MOUSEMOVE, MK_LBUTTON, MAKELPARAM(12, 450));
      assert(desktop.toolbar_edge_ == DesktopWindows::ToolbarEdge::Left);
      assert(view.toolbar_rect.left == 0);
      AssertVerticalToolbar(view);
      SendMessageW(view.window, WM_MOUSEMOVE, MK_LBUTTON, MAKELPARAM(600, 450));
      assert(desktop.toolbar_floating_);
      SendMessageW(view.window, WM_LBUTTONUP, 0, MAKELPARAM(600, 450));
      assert(!desktop.toolbar_dragging_);
      const POINT regrab{view.handle_rect.left + 4, view.handle_rect.top + 4};
      SendMessageW(view.window, WM_LBUTTONDOWN, MK_LBUTTON, MAKELPARAM(regrab.x, regrab.y));
      assert(desktop.toolbar_dragging_);
      SendMessageW(view.window, WM_MOUSEMOVE, MK_LBUTTON, MAKELPARAM(1188, 450));
      assert(desktop.toolbar_edge_ == DesktopWindows::ToolbarEdge::Right);
      assert(view.toolbar_rect.right == 1200);
      AssertVerticalToolbar(view);
      SendMessageW(view.window, WM_LBUTTONUP, 0, MAKELPARAM(1188, 450));
      assert(!desktop.toolbar_dragging_);
      // The classic centered top bar remains the default placement.
      desktop.toolbar_edge_ = DesktopWindows::ToolbarEdge::Top;
      desktop.toolbar_floating_ = false;
      desktop.LayoutToolbar(view);
      assert(view.toolbar_rect.top == 0 && view.toolbar_rect.bottom > 0);
      commands.clear();

      SendMessageW(view.window, WM_COMMAND, MAKEWPARAM(103, BN_CLICKED), reinterpret_cast<LPARAM>(view.buttons[2]));
      MSG choice{};
      assert(PeekMessageW(&choice, view.window, WM_APP+7, WM_APP+7, PM_REMOVE));
      assert(choice.wParam == 3);

      // Unequal-height monitors must show their own top-origin canvas region.
      auto bands = std::make_shared<Pixels>();
      bands->width = 100; bands->height = 100;
      for (int y = 0; y < 100; ++y) for (int x = 0; x < 100; ++x) {
        const uint8_t red = y < 30 ? 255 : 0, blue = y >= 30 ? 255 : 0;
        bands->bgra.insert(bands->bgra.end(), {blue, 0, red, 255});
      }
      feed->latest = bands;
      desktop.layout_ = ad_desktop::Arrange({{0, 0, 50, 30}, {50, 30, 50, 70}});
      view.region = desktop.layout_.monitors[0];
      SetWindowPos(view.window, nullptr, 0, 0, 50, 30, SWP_NOZORDER | SWP_NOACTIVATE);
      desktop.Paint(view);
      assert(GetPixel(view.buffer, 25, 15) == RGB(255, 0, 0));
      auto& second = *desktop.views_[1];
      second.region = desktop.layout_.monitors[1];
      SetWindowPos(second.window, nullptr, 0, 0, 50, 70, SWP_NOZORDER | SWP_NOACTIVATE);
      desktop.Paint(second);
      assert(GetPixel(second.buffer, 25, 35) == RGB(0, 0, 255));

      desktop.keyboard_id_ = CurrentKeyboardId();
      SendMessageW(view.window, WM_KEYDOWN, 'A', 30LL << 16);
      assert(desktop.keys_.count(30));
      fail_send = true;
      SendMessageW(view.window, WM_KEYUP, 'A', 30LL << 16);
      assert(desktop.keys_.count(30)); // Failed key-up remains retryable.
      fail_send = false;
      desktop.ReleaseInput();
      assert(desktop.keys_.empty());
      const auto before_repeat = commands.size();
      SendMessageW(view.window, WM_KEYDOWN, 'A', (30LL << 16) | (1LL << 30));
      assert(desktop.keys_.empty() && commands.size() == before_repeat);
      commands.clear();

      assert(desktop.InstallExitShortcut());
      assert(DesktopWindows::keyboard_hook_ != nullptr);
      HWND one = view.window, two = desktop.views_[1]->window;
      assert(!desktop.ExitShortcut(VK_ESCAPE, true, true, GetDesktopWindow()));
      assert(!desktop.ExitShortcut(VK_ESCAPE, false, true, one));
      assert(!desktop.ExitShortcut('A', true, true, one));
      desktop.configured_ = true;
      // Resizing the tab behind an unfocused presentation only changes the
      // return target; it must not collapse the active monitor arrangement.
      assert(!desktop.HasFocus());
      assert(desktop.SetTabSize(900, 520));
      assert(commands.empty());
      desktop.keys_ = {29, 56};
      desktop.buttons_ = {1};
      assert(desktop.ExitShortcut(VK_ESCAPE, true, true, two));
      MSG message{};
      assert(PeekMessageW(&message, two, WM_APP+8, WM_APP+8, PM_REMOVE));
      DispatchMessageW(&message);
      assert(!IsWindow(one) && !IsWindow(two));
      assert(desktop.views_.empty() && !feed->presenting.load());
      assert(DesktopWindows::keyboard_hook_ == nullptr);
      assert(DesktopWindows::presenters_.empty());
      assert(desktop.keys_.empty() && desktop.buttons_.empty());
      assert(commands.size() == 4); // Release input and restore the current tab and one monitor.
      for (size_t i = 0; i < 3; ++i) assert(commands[i].find("\"pressed\":false") != std::string::npos);
      assert(commands.back() == "{\"type\":\"monitorLayout\",\"width\":900,\"height\":520,\"monitors\":[{\"x\":0,\"y\":0,\"width\":900,\"height\":520}]}");
      desktop.Close();
      assert(commands.size() == 4); // Return is idempotent.
      desktop.configured_ = true;
      desktop.Close(false);
      assert(commands.size() == 4); // Disconnect keeps the retained host display.
      assert(desktop.SetTabSize(1200, 700));
      assert(commands.size() == 5);
      assert(desktop.SetTabSize(1200, 700));
      assert(commands.size() == 5); // Video frame updates do not resend the layout.
      assert(desktop.SetTabSize(1200, 700, true));
      assert(commands.size() == 6);

      // Monitor re-detection rebuilds the presentation without returning to
      // the tab and sends exactly one fresh layout to the host.
      commands.clear();
      feed->presenting.store(true);
      desktop.all_monitors_ = true;
      desktop.layout_ = ad_desktop::Arrange({{-4000, -4000, 40, 30}});
      {
        auto probe = std::make_unique<DesktopWindows::View>();
        probe->owner = &desktop; probe->region = desktop.layout_.monitors[0]; probe->menu = false;
        probe->window = CreateWindowW(klass.lpszClassName, L"Test", WS_POPUP,
            -4000, -4000, 40, 30, nullptr, nullptr, klass.hInstance, probe.get());
        assert(probe->window);
        desktop.views_.push_back(std::move(probe));
      }
      const HWND probe_window = desktop.views_.front()->window;
      SendMessageW(probe_window, WM_DISPLAYCHANGE, 0, 0);
      SendMessageW(probe_window, WM_DISPLAYCHANGE, 0, 0);
      assert(desktop.redetect_pending_); // one debounced rebuild is scheduled
      commands.clear();
      std::vector<DesktopWindows::Monitor> changed = {
          {RECT{-4000, -4000, -3600, -3700}, true},
          {RECT{-3600, -3700, -3200, -3400}, true}};
      fail_send = true;
      assert(!desktop.ApplyMonitors(changed));
      assert(IsWindow(probe_window) && desktop.views_.size() == 1);
      assert(feed->presenting.load()); // Failed host configuration preserves the usable view.
      fail_send = false;
      assert(desktop.ApplyMonitors(changed));
      assert(desktop.redetect_pending_); // only timer expiry clears the flag
      assert(feed->presenting.load());   // the session is never torn down
      assert(desktop.views_.size() == 2 && desktop.views_[0]->toolbar);
      assert(IsWindow(desktop.views_[0]->window) && !IsWindow(probe_window));
      size_t layouts = 0;
      for (const auto& command : commands)
        if (command.find("\"monitorLayout\"") != std::string::npos) ++layouts;
      assert(layouts == 1); // the tab layout is never sent during a rebuild
      assert(commands.back().find("\"width\":800") != std::string::npos);
      // An impossible arrangement keeps the last good presentation untouched.
      commands.clear();
      desktop.ApplyMonitors({});
      std::vector<DesktopWindows::Monitor> too_many(17, {RECT{-5000, -5000, -4900, -4900}, true});
      desktop.ApplyMonitors(too_many);
      assert(commands.empty() && desktop.views_.size() == 2 && feed->presenting.load());
      desktop.redetect_pending_ = false;
      desktop.all_monitors_ = false;
      // A caller using logical coordinates and one using physical pixels
      // must see the same topology, including on scaled/mixed-DPI monitors.
      const auto caller_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
      assert(caller_dpi);
      const auto unaware_snapshot = DesktopWindows::Snapshot(desktop.EnumerateMonitors(true, nullptr));
      assert(!unaware_snapshot.empty());
      assert(AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_UNAWARE));
      {
        PhysicalDpiScope physical_dpi;
        const auto physical_snapshot = DesktopWindows::Snapshot(desktop.EnumerateMonitors(true, nullptr));
        assert(DesktopWindows::SameSnapshot(unaware_snapshot, physical_snapshot));
        desktop.monitor_snapshot_ = physical_snapshot;
      }
      desktop.CheckMonitorSnapshot();
      assert(!desktop.redetect_pending_);
      assert(AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), DPI_AWARENESS_CONTEXT_UNAWARE));
      SetThreadDpiAwarenessContext(caller_dpi);
    }
    UnregisterClassW(klass.lpszClassName, klass.hInstance);
    commands.clear();
    return GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS);
  }
};
}
int main() {
  // The first use initializes Windows' cached stock font/brush resources.
  const DWORD steady = DesktopWindowsTest::Run();
  assert(DesktopWindowsTest::Run() == steady);
}
