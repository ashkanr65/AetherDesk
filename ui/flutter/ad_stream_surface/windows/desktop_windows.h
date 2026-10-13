// Included after Feed: all view windows share its immutable decoded snapshot.
#include "desktop_log.h"
#include "desktop_platform.h"
#include <windowsx.h>
#include <set>
#include <sstream>

using RemoteCommand = int (*)(const char*, const char*);

class PhysicalDpiScope {
 public:
  PhysicalDpiScope() : previous_(SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)) {}
  ~PhysicalDpiScope() { if (previous_) SetThreadDpiAwarenessContext(previous_); }
 private:
  DPI_AWARENESS_CONTEXT previous_;
};

uint32_t CurrentKeyboardId() {
  wchar_t name[KL_NAMELENGTH]{};
  if (GetKeyboardLayoutNameW(name)) return static_cast<uint32_t>(wcstoul(name, nullptr, 16));
  return static_cast<uint32_t>(reinterpret_cast<uintptr_t>(GetKeyboardLayout(0)) & 0xffff);
}

class DesktopWindows {
  enum class ToolbarEdge { Top, Bottom, Left, Right };
  struct Monitor { RECT rect; bool primary; };

 public:
  DesktopWindows(std::shared_ptr<Feed> feed, HWND parent, RemoteCommand command)
      : feed_(std::move(feed)), parent_(GetAncestor(parent, GA_ROOT)), command_(command) {
    std::lock_guard<std::mutex> lock(feed_->mutex);
    if (feed_->latest) { tab_width_ = static_cast<uint32_t>(feed_->latest->width); tab_height_ = static_cast<uint32_t>(feed_->latest->height); }
  }
  ~DesktopWindows() { Close(false); }
  bool SetTabSize(uint32_t width, uint32_t height, bool force = false) {
    const bool changed = !tab_size_known_ || tab_width_ != width || tab_height_ != height;
    tab_size_known_ = true;
    tab_width_ = width; tab_height_ = height;
    // Layout changes behind fullscreen update the return target only. Losing
    // focus does not mean the native presentation has ended.
    if (!views_.empty() || (!changed && !force)) return true;
    return Configure(TabLayout());
  }
  bool Open(bool all_monitors, HWND reference = nullptr) {
    // Enumerate and create views in the same physical-pixel coordinate space,
    // even when a plugin callback arrives with a DPI-virtualized thread context.
    PhysicalDpiScope physical_dpi;
    // Resolve the reference before Close destroys the toolbar's view window.
    std::vector<Monitor> monitors = EnumerateMonitors(all_monitors, reference ? reference : parent_);
    Close(false);
    all_monitors_ = all_monitors;
    parent_monitor_ = MonitorFromWindow(parent_, MONITOR_DEFAULTTONEAREST);
    follow_parent_ = false;
    std::vector<ad_desktop::Rect> rectangles;
    for (const auto& m : monitors) rectangles.push_back({static_cast<int>(m.rect.left), static_cast<int>(m.rect.top), static_cast<int>(m.rect.right-m.rect.left), static_cast<int>(m.rect.bottom-m.rect.top)});
    layout_ = ad_desktop::Arrange(rectangles);
    if (layout_.monitors.empty() || !Configure(layout_)) { Close(); return false; }
    configured_ = true;
    monitor_snapshot_ = Snapshot(EnumerateMonitors(true, nullptr));
    {
      char event[128];
      std::snprintf(event, sizeof(event), "present open all=%d monitors=%zu",
                    all_monitors_ ? 1 : 0, monitors.size());
      AdNativeLog("info", event);
    }
    if (!RegisterViewClass()) { Close(); return false; }
    feed_->presenting.store(true);
    PreparePixels();
    if (!CreateViews(monitors)) { Close(); return false; }
    if (!InstallExitShortcut()) { Close(); return false; }
    SyncKeyboard();
    SetForegroundWindow(views_.front()->window); SetFocus(views_.front()->window);
    return true;
  }
  // Rebuild the presentation for the current monitors without ever returning
  // to the tab: the host keeps streaming and receives exactly one new layout.
  void Redetect() {
    if (views_.empty()) return;
    PhysicalDpiScope physical_dpi;
    const HWND reference = follow_parent_ && IsWindow(parent_) ? parent_ : views_.front()->window;
    std::vector<Monitor> monitors = EnumerateMonitors(all_monitors_, all_monitors_ ? nullptr : reference);
    if (ApplyMonitors(std::move(monitors))) {
      monitor_snapshot_ = Snapshot(EnumerateMonitors(true, nullptr));
      follow_parent_ = false;
    }
  }
  bool ApplyMonitors(std::vector<Monitor> monitors) {
    if (views_.empty() || monitors.empty()) return false;
    PhysicalDpiScope physical_dpi;
    std::vector<ad_desktop::Rect> rectangles;
    for (const auto& m : monitors) rectangles.push_back({static_cast<int>(m.rect.left), static_cast<int>(m.rect.top), static_cast<int>(m.rect.right-m.rect.left), static_cast<int>(m.rect.bottom-m.rect.top)});
    ad_desktop::Layout next = ad_desktop::Arrange(rectangles);
    // An impossible arrangement keeps the last good layout instead of ending.
    if (next.monitors.empty()) return false;
    if (SameArrangement(next)) { ResyncViewRects(monitors); return true; }
    // The new layout is sent before the views are rebuilt; on failure the
    // existing views and the host layout stay consistent.
    std::vector<std::unique_ptr<View>> replacements;
    if (!CreateViewsInto(monitors, next, replacements) || !Configure(next)) {
      for (auto& view : replacements) if (IsWindow(view->window)) DestroyWindow(view->window);
      AdNativeLog("warn", "monitor refresh failed; keeping previous presentation");
      return false;
    }
    ReleaseInput();
    for (auto& view : views_) { if (IsWindow(view->window)) DestroyWindow(view->window); }
    views_ = std::move(replacements);
    layout_ = next;
    for (auto& view : views_) ShowWindow(view->window, SW_SHOW);
    AdNativeLog("info", "monitor layout rebuilt after display change");
    SyncKeyboard();
    if (!views_.empty()) { SetForegroundWindow(views_.front()->window); SetFocus(views_.front()->window); }
    return true;
  }
  void ScheduleRedetect() {
    if (views_.empty()) return;
    redetect_pending_ = true;
    // Windows sends several display changes per mode switch; restart one timer.
    SetTimer(views_.front()->window, 2, 400, nullptr);
  }
  void CheckMonitorSnapshot() {
    PhysicalDpiScope physical_dpi;
    std::vector<Monitor> monitors = EnumerateMonitors(true, nullptr);
    const HMONITOR parent_monitor = MonitorFromWindow(parent_, MONITOR_DEFAULTTONEAREST);
    if (!all_monitors_ && IsWindow(parent_) && parent_monitor != parent_monitor_) follow_parent_ = true;
    parent_monitor_ = parent_monitor;
    if (!redetect_pending_ && (follow_parent_ || !SameSnapshot(Snapshot(monitors), monitor_snapshot_))) ScheduleRedetect();
  }
  void Close(bool restore_parent = true) {
    AdNativeLog("info", restore_parent ? "present close" : "present close (keep parent)");
    presenters_.erase(this);
    if (presenters_.empty() && keyboard_hook_) {
      UnhookWindowsHookEx(keyboard_hook_); keyboard_hook_ = nullptr;
    }
    ReleaseInput();
    feed_->presenting.store(false);
    redetect_pending_ = false;
    toolbar_dragging_ = false;
    slow_ticks_ = 0;
    for (auto& view : views_) { if (IsWindow(view->window)) DestroyWindow(view->window); }
    views_.clear();
    if (restore_parent && configured_) {
      configured_ = false;
      if (tab_width_ && tab_height_) {
        Configure(TabLayout());
      }
    }
    if (restore_parent && IsWindow(parent_)) { ShowWindow(parent_, IsIconic(parent_) ? SW_RESTORE : SW_SHOW); SetForegroundWindow(parent_); }
  }
  bool HasFocus() const {
    const HWND foreground = GetForegroundWindow();
    for (const auto& view : views_) if (foreground == view->window) return true;
    return false;
  }

 private:
  friend struct DesktopWindowsTest;
  struct View {
    DesktopWindows* owner;
    HWND window = nullptr;
    ad_desktop::Rect region{};
    uint64_t sequence = 0;
    bool menu = true;
    bool toolbar = false;
    HWND buttons[3]{};
    RECT toolbar_rect{};
    RECT handle_rect{};
    HDC buffer = nullptr;
    HBITMAP bitmap = nullptr;
    HGDIOBJ previous_bitmap = nullptr;
    int width = 0, height = 0;
    ~View() { ResetBuffer(); }
    void ResetBuffer() {
      if (buffer && previous_bitmap) SelectObject(buffer, previous_bitmap);
      if (bitmap) DeleteObject(bitmap);
      if (buffer) DeleteDC(buffer);
      buffer = nullptr; bitmap = nullptr; previous_bitmap = nullptr;
    }
    bool PrepareBuffer(HDC target, int w, int h) {
      if (buffer && width == w && height == h) return true;
      ResetBuffer();
      if (w <= 0 || h <= 0) return false;
      buffer = CreateCompatibleDC(target);
      bitmap = CreateCompatibleBitmap(target, w, h);
      if (!buffer || !bitmap) { ResetBuffer(); return false; }
      previous_bitmap = SelectObject(buffer, bitmap);
      width = w; height = h;
      return true;
    }
  };
  // The OS handles Ctrl+Alt+Esc before a normal window key message. Intercept
  // only this chord, only for a focused presentation; do no I/O in the hook.
  static LRESULT CALLBACK KeyboardHook(int code, WPARAM wp, LPARAM lp) {
    if (code == HC_ACTION && (wp == WM_KEYDOWN || wp == WM_SYSKEYDOWN)) {
      const auto& key = *reinterpret_cast<const KBDLLHOOKSTRUCT*>(lp);
      for (auto* owner : presenters_) {
        if (owner->ExitShortcut(key.vkCode, (GetAsyncKeyState(VK_CONTROL) & 0x8000) != 0,
            (GetAsyncKeyState(VK_MENU) & 0x8000) != 0, GetForegroundWindow())) return 1;
      }
    }
    return CallNextHookEx(keyboard_hook_, code, wp, lp);
  }
  bool ExitShortcut(DWORD key, bool control, bool alt, HWND foreground) {
    if (key != VK_ESCAPE || !control || !alt) return false;
    for (const auto& view : views_) {
      if (view->window == foreground) {
        PostMessageW(foreground, WM_APP+8, 0, 0);
        return true;
      }
    }
    return false;
  }
  bool InstallExitShortcut() {
    if (!keyboard_hook_) {
      HMODULE module = nullptr;
      if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
          reinterpret_cast<LPCWSTR>(&KeyboardHook), &module)) return false;
      keyboard_hook_ = SetWindowsHookExW(WH_KEYBOARD_LL, KeyboardHook, module, 0);
    }
    if (!keyboard_hook_) return false;
    presenters_.insert(this);
    return true;
  }
  inline static HHOOK keyboard_hook_ = nullptr;
  inline static std::set<DesktopWindows*> presenters_;
  bool Send(const std::string& json) { return command_ && command_(feed_->session.c_str(), json.c_str()) == 0; }
  std::vector<Monitor> EnumerateMonitors(bool all_monitors, HWND reference) {
    // Snapshots must have the same units regardless of the caller's DPI
    // awareness. Otherwise a stable scaled monitor looks like a topology change.
    // Keep the guard here so tests and future callers cannot omit it.
    PhysicalDpiScope physical_dpi;
    std::vector<Monitor> monitors;
    if (all_monitors) {
      EnumDisplayMonitors(nullptr, nullptr, [](HMONITOR monitor, HDC, LPRECT, LPARAM data) -> BOOL {
        auto& list = *reinterpret_cast<std::vector<Monitor>*>(data);
        MONITORINFO info{}; info.cbSize = sizeof(info);
        if (GetMonitorInfoW(monitor, &info)) list.push_back({info.rcMonitor, (info.dwFlags & MONITORINFOF_PRIMARY) != 0});
        return TRUE;
      }, reinterpret_cast<LPARAM>(&monitors));
      std::stable_sort(monitors.begin(), monitors.end(), [](const Monitor& a, const Monitor& b) { return a.primary && !b.primary; });
    } else {
      // A disappeared target monitor falls back to the nearest remaining one.
      const HMONITOR selected = MonitorFromWindow(reference, MONITOR_DEFAULTTONEAREST);
      MONITORINFO info{}; info.cbSize = sizeof(info);
      if (GetMonitorInfoW(selected, &info)) monitors.push_back({info.rcMonitor, true});
    }
    return monitors;
  }
  static std::vector<RECT> Snapshot(const std::vector<Monitor>& monitors) {
    std::vector<RECT> rects;
    for (const auto& monitor : monitors) rects.push_back(monitor.rect);
    return rects;
  }
  static bool SameSnapshot(const std::vector<RECT>& left, const std::vector<RECT>& right) {
    if (left.size() != right.size()) return false;
    for (size_t i = 0; i < left.size(); ++i) {
      if (left[i].left != right[i].left || left[i].top != right[i].top ||
          left[i].right != right[i].right || left[i].bottom != right[i].bottom) return false;
    }
    return true;
  }
  static bool RegisterViewClass() {
    WNDCLASSW klass{}; klass.lpfnWndProc = WindowProc; klass.hInstance = GetModuleHandleW(nullptr);
    klass.hCursor = LoadCursorW(nullptr, IDC_ARROW); klass.lpszClassName = L"AetherDeskRemoteMonitor";
    return RegisterClassW(&klass) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS;
  }
  bool CreateViews(const std::vector<Monitor>& monitors) {
    if (!CreateViewsInto(monitors, layout_, views_)) return false;
    for (auto& view : views_) ShowWindow(view->window, SW_SHOW);
    return true;
  }
  bool CreateViewsInto(const std::vector<Monitor>& monitors, const ad_desktop::Layout& layout,
                       std::vector<std::unique_ptr<View>>& output) {
    if (!RegisterViewClass()) return false;
    for (size_t i = 0; i < monitors.size(); ++i) {
      auto view = std::make_unique<View>(); view->owner = this; view->region = layout.monitors[i];
      view->toolbar = i == 0; // Primary monitor is sorted first, independent of virtual-screen origin.
      const auto& r = monitors[i].rect;
      view->window = CreateWindowExW(WS_EX_APPWINDOW, L"AetherDeskRemoteMonitor", L"AetherDesk Remote Desktop",
          WS_POPUP | WS_CLIPCHILDREN, r.left, r.top, r.right-r.left, r.bottom-r.top, nullptr, nullptr, GetModuleHandleW(nullptr), view.get());
      if (!view->window) return false;
      if (view->toolbar) CreateToolbar(*view);
      SetTimer(view->window, 1, 16, nullptr);
      output.push_back(std::move(view));
    }
    return !output.empty();
  }
  bool SameArrangement(const ad_desktop::Layout& next) const {
    if (next.width != layout_.width || next.height != layout_.height ||
        next.monitors.size() != layout_.monitors.size()) return false;
    for (size_t i = 0; i < next.monitors.size(); ++i) {
      const auto& a = next.monitors[i];
      const auto& b = layout_.monitors[i];
      if (a.x != b.x || a.y != b.y || a.width != b.width || a.height != b.height) return false;
    }
    return true;
  }
  void ResyncViewRects(const std::vector<Monitor>& monitors) {
    for (size_t i = 0; i < views_.size() && i < monitors.size(); ++i) {
      const RECT& r = monitors[i].rect;
      SetWindowPos(views_[i]->window, nullptr, r.left, r.top, r.right-r.left, r.bottom-r.top, SWP_NOZORDER | SWP_NOACTIVATE);
      LayoutToolbar(*views_[i]);
    }
  }
  ad_desktop::Layout TabLayout() const {
    const int width = static_cast<int>(tab_width_), height = static_cast<int>(tab_height_);
    return {width, height, {{0, 0, width, height}}};
  }
  bool Configure(const ad_desktop::Layout& layout) {
    {
      char event[128];
      std::snprintf(event, sizeof(event), "monitorLayout sent %dx%d monitors=%zu",
                    layout.width, layout.height, layout.monitors.size());
      AdNativeLog("info", event);
    }
    std::ostringstream json;
    json << "{\"type\":\"monitorLayout\",\"width\":" << layout.width << ",\"height\":" << layout.height << ",\"monitors\":[";
    for (size_t i = 0; i < layout.monitors.size(); ++i) {
      const auto& r = layout.monitors[i]; if (i) json << ',';
      json << "{\"x\":" << r.x << ",\"y\":" << r.y << ",\"width\":" << r.width << ",\"height\":" << r.height << '}';
    }
    json << "]}"; return Send(json.str());
  }
  void SyncKeyboard() {
    const uint32_t id = CurrentKeyboardId();
    if (id == keyboard_id_) return;
    const auto mapping = ad_desktop::KeyboardForId(id);
    if (mapping.layout) ReleaseInput();
    if (mapping.layout && Send(std::string("{\"type\":\"keyboardLayout\",\"layout\":\"") + mapping.layout + "\",\"variant\":\"" + mapping.variant + "\"}")) keyboard_id_ = id;
  }
  bool Key(int code, bool down) {
    if (!code) return false;
    const unsigned locks = 0x8000 | ((GetKeyState(VK_NUMLOCK) & 1) ? 0x10 : 0) | ((GetKeyState(VK_CAPITAL) & 1) ? 0x20 : 0);
    return Send("{\"type\":\"input\",\"event\":{\"Keyboard\":{\"scan_code\":" + std::to_string(code) + ",\"pressed\":" + (down ? "true" : "false") + ",\"modifiers\":" + std::to_string(locks) + ",\"repeat\":false}}}");
  }
  void ReleaseInput() {
    for (auto it = keys_.begin(); it != keys_.end();) {
      if (Key(*it, false)) it = keys_.erase(it); else ++it;
    }
    for (auto it = buttons_.begin(); it != buttons_.end();) {
      if (Button(*it, false)) it = buttons_.erase(it); else ++it;
    }
    if (GetCapture()) ReleaseCapture();
  }
  void Move(View& view, int x, int y) {
    RECT client{}; GetClientRect(view.window, &client);
    pointer_x_ = std::clamp(view.region.x + x*view.region.width/std::max(1L, client.right), 0L, static_cast<long>(layout_.width-1));
    pointer_y_ = std::clamp(view.region.y + y*view.region.height/std::max(1L, client.bottom), 0L, static_cast<long>(layout_.height-1));
    Send("{\"type\":\"input\",\"event\":{\"PointerMove\":{\"x\":" + std::to_string(pointer_x_) + ",\"y\":" + std::to_string(pointer_y_) + ",\"monitor\":0}}}");
  }
  bool Button(int button, bool down) {
    return Send("{\"type\":\"input\",\"event\":{\"PointerButton\":{\"button\":" + std::to_string(button) + ",\"pressed\":" + (down ? "true" : "false") + ",\"x\":" + std::to_string(pointer_x_) + ",\"y\":" + std::to_string(pointer_y_) + "}}}");
  }
  static int Scale(HWND window, int value) { return MulDiv(value, static_cast<int>(GetDpiForWindow(window)), 96); }
  // Toolbar geometry: docked along an edge (top/bottom horizontal, left/right
  // vertical) or floating where the user dropped it.
  RECT ToolbarRect(View& view, const RECT& client) const {
    const int thickness = std::min(static_cast<int>(client.bottom), Scale(view.window, 38));
    const int wide = std::min(static_cast<int>(client.right), Scale(view.window, 540));
    if (toolbar_floating_) {
      const int x = std::clamp<int>(toolbar_position_.x, 0, std::max(0, static_cast<int>(client.right) - wide));
      const int y = std::clamp<int>(toolbar_position_.y, 0, std::max(0, static_cast<int>(client.bottom) - thickness));
      return {x, y, x + wide, y + thickness};
    }
    switch (toolbar_edge_) {
      case ToolbarEdge::Top: {
        const int left = (static_cast<int>(client.right) - wide) / 2;
        return {left, 0, left + wide, thickness};
      }
      case ToolbarEdge::Bottom: {
        const int left = (static_cast<int>(client.right) - wide) / 2;
        return {left, static_cast<int>(client.bottom) - thickness, left + wide, static_cast<int>(client.bottom)};
      }
      case ToolbarEdge::Left: {
        const int width = std::min(static_cast<int>(client.right), Scale(view.window, 180));
        const int height = std::min(static_cast<int>(client.bottom), thickness * 4);
        const int top = (static_cast<int>(client.bottom) - height) / 2;
        return {0, top, width, top + height};
      }
      case ToolbarEdge::Right: {
        const int width = std::min(static_cast<int>(client.right), Scale(view.window, 180));
        const int height = std::min(static_cast<int>(client.bottom), thickness * 4);
        const int top = (static_cast<int>(client.bottom) - height) / 2;
        return {static_cast<int>(client.right) - width, top, static_cast<int>(client.right), top + height};
      }
    }
    return {0, 0, wide, thickness};
  }
  void LayoutToolbar(View& view) {
    if (!view.toolbar) return;
    RECT client{}; GetClientRect(view.window, &client);
    view.toolbar_rect = ToolbarRect(view, client);
    const int handle_size = Scale(view.window, 44);
    const bool vertical = !toolbar_floating_ &&
        (toolbar_edge_ == ToolbarEdge::Left || toolbar_edge_ == ToolbarEdge::Right);
    const int left = view.toolbar_rect.left, top = view.toolbar_rect.top;
    const int width = view.toolbar_rect.right - left, height = view.toolbar_rect.bottom - top;
    const int handle = std::min(handle_size, vertical ? height : width);
    if (vertical) {
      view.handle_rect = {left, top, left + width, top + handle};
      const int body = height - handle;
      for (int i = 0; i < 3; ++i) {
        const int y = top + handle + body * i / 3;
        MoveWindow(view.buttons[i], left, y, width, top + handle + body * (i + 1) / 3 - y, TRUE);
      }
    } else {
      view.handle_rect = {left, top, left + handle, top + height};
      for (int i = 0; i < 3; ++i) {
        const int x = left + handle + (width - handle) * i / 3;
        MoveWindow(view.buttons[i], x, top, left + handle + (width - handle) * (i + 1) / 3 - x, height, TRUE);
      }
    }
  }
  // The hamburger handle belongs to the parent so it can own the drag; the
  // three action buttons remain ordinary child windows.
  static void DrawToolbarHandle(HDC dc, const RECT& rect) {
    HBRUSH background = CreateSolidBrush(RGB(30, 35, 45));
    FillRect(dc, &rect, background); DeleteObject(background);
    HBRUSH border = CreateSolidBrush(RGB(75, 85, 96));
    FrameRect(dc, &rect, border); DeleteObject(border);
    HBRUSH line = CreateSolidBrush(RGB(245, 245, 245));
    const int cx = (rect.left + rect.right) / 2, cy = (rect.top + rect.bottom) / 2;
    const int half = std::max<int>(6, (rect.right - rect.left) / 4);
    const int gap = std::max<int>(3, (rect.bottom - rect.top) / 8);
    for (int i = -1; i <= 1; ++i) {
      RECT stroke = {cx - half, cy + i * gap - 1, cx + half, cy + i * gap + 1};
      FillRect(dc, &stroke, line);
    }
    DeleteObject(line);
  }
  void BeginToolbarDrag(View& view, int x, int y) {
    ReleaseInput();
    toolbar_dragging_ = true;
    toolbar_drag_offset_ = {x - view.toolbar_rect.left, y - view.toolbar_rect.top};
    SetCapture(view.window);
  }
  void DragToolbar(View& view, int x, int y) {
    RECT client{}; GetClientRect(view.window, &client);
    const int left = x - toolbar_drag_offset_.x, top = y - toolbar_drag_offset_.y;
    // Within the edge margin the bar snaps: horizontal on top/bottom,
    // vertical on the left/right side of this screen.
    const int margin = Scale(view.window, 64);
    const int distances[] = {y, static_cast<int>(client.bottom) - y, x,
                             static_cast<int>(client.right) - x};
    int nearest = 0;
    for (int i = 1; i < 4; ++i) if (distances[i] < distances[nearest]) nearest = i;
    if (distances[nearest] <= margin) {
      toolbar_floating_ = false;
      toolbar_edge_ = static_cast<ToolbarEdge>(nearest);
    } else {
      toolbar_floating_ = true;
      toolbar_position_ = {left, top};
    }
    LayoutToolbar(view);
    InvalidateRect(view.window, nullptr, FALSE);
  }
  void EndToolbarDrag(View& view) {
    if (!toolbar_dragging_) return;
    toolbar_dragging_ = false;
    if (GetCapture() == view.window) ReleaseCapture();
    LayoutToolbar(view);
    InvalidateRect(view.window, nullptr, FALSE);
  }
  void ResyncViewRect(View& view) {
    const HMONITOR monitor = MonitorFromWindow(view.window, MONITOR_DEFAULTTONEAREST);
    MONITORINFO info{}; info.cbSize = sizeof(info);
    if (!IsWindow(view.window) || !GetMonitorInfoW(monitor, &info)) return;
    const RECT& r = info.rcMonitor;
    RECT current{}; GetWindowRect(view.window, &current);
    if (current.left != r.left || current.top != r.top ||
        current.right != r.right || current.bottom != r.bottom) {
      SetWindowPos(view.window, nullptr, r.left, r.top, r.right - r.left, r.bottom - r.top,
          SWP_NOZORDER | SWP_NOACTIVATE);
    }
    LayoutToolbar(view);
  }
  void CreateToolbar(View& view) {
    static const wchar_t* labels[] = {L"Return to tab", L"Use this monitor", L"Use all monitors"};
    for (int i = 0; i < 3; ++i) {
      view.buttons[i] = CreateWindowExW(0, L"BUTTON", labels[i], WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
          0, 0, 1, 1, view.window, reinterpret_cast<HMENU>(static_cast<INT_PTR>(101 + i)), GetModuleHandleW(nullptr), nullptr);
    }
    LayoutToolbar(view);
  }
  static void DrawToolbarButton(const DRAWITEMSTRUCT& item) {
    if (item.CtlID < 101 || item.CtlID > 103) return;
    const int index = static_cast<int>(item.CtlID - 101);
    static const wchar_t* glyphs[] = {L"\xE73F", L"\xE7F4", L"\xE8AE"};
    HDC dc = item.hDC;
    HBRUSH background = CreateSolidBrush((item.itemState & ODS_SELECTED) ? RGB(58, 78, 90) : RGB(30, 35, 45));
    FillRect(dc, &item.rcItem, background); DeleteObject(background);
    HBRUSH border = CreateSolidBrush(RGB(75, 85, 96));
    FrameRect(dc, &item.rcItem, border); DeleteObject(border);
    SetBkMode(dc, TRANSPARENT); SetTextColor(dc, RGB(245, 245, 245));
    HFONT icon = CreateFontW(-Scale(item.hwndItem, 16), 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_PITCH, L"Segoe MDL2 Assets");
    auto old = SelectObject(dc, icon);
    RECT symbol = item.rcItem; symbol.left += Scale(item.hwndItem, 10); symbol.right = symbol.left + Scale(item.hwndItem, 20);
    DrawTextW(dc, glyphs[index], -1, &symbol, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(dc, old); DeleteObject(icon);
    HFONT font = CreateFontW(-Scale(item.hwndItem, 13), 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, DEFAULT_PITCH, L"Segoe UI");
    old = SelectObject(dc, font);
    RECT text = item.rcItem; text.left = symbol.right + Scale(item.hwndItem, 6); text.right -= Scale(item.hwndItem, 6);
    wchar_t label[64]{}; GetWindowTextW(item.hwndItem, label, 64);
    if (text.right > text.left) DrawTextW(dc, label, -1, &text, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    SelectObject(dc, old); DeleteObject(font);
    if (item.itemState & ODS_FOCUS) DrawFocusRect(dc, &item.rcItem);
  }
  void PreparePixels() {
    std::lock_guard<std::mutex> lock(feed_->mutex);
    if (!feed_->latest || !feed_->latest->bgra.empty()) return;
    // An idle host may not send another frame after opening fullscreen at the
    // same resolution. Convert the existing Flutter snapshot once for GDI.
    auto pixels = std::make_shared<Pixels>(*feed_->latest);
    pixels->bgra.resize(pixels->rgba.size());
    for (size_t i = 0; i + 3 < pixels->rgba.size(); i += 4) {
      pixels->bgra[i] = pixels->rgba[i + 2];
      pixels->bgra[i + 1] = pixels->rgba[i + 1];
      pixels->bgra[i + 2] = pixels->rgba[i];
      pixels->bgra[i + 3] = pixels->rgba[i + 3];
    }
    feed_->latest = std::move(pixels);
  }
  void Paint(View& view) {
    PAINTSTRUCT paint{}; HDC target = BeginPaint(view.window, &paint);
    RECT client{}; GetClientRect(view.window, &client);
    if (!view.PrepareBuffer(target, client.right, client.bottom)) {
      EndPaint(view.window, &paint); return;
    }
    HDC dc = view.buffer;
    // Compose the frame and menu off-screen; publish one completed image.
    FillRect(dc, &client, static_cast<HBRUSH>(GetStockObject(BLACK_BRUSH)));
    std::shared_ptr<const Pixels> pixels;
    { std::lock_guard<std::mutex> lock(feed_->mutex); pixels = feed_->latest; }
    if (pixels && !pixels->bgra.empty()) {
      BITMAPINFO info{}; info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
      info.bmiHeader.biWidth = static_cast<LONG>(pixels->width); info.bmiHeader.biHeight = -static_cast<LONG>(pixels->height);
      info.bmiHeader.biPlanes = 1; info.bmiHeader.biBitCount = 32; info.bmiHeader.biCompression = BI_RGB;
      // While resize is in flight, proportionally crop the previous canvas.
      const int sx = static_cast<int>(static_cast<int64_t>(view.region.x)*pixels->width/layout_.width);
      const int sy = static_cast<int>(static_cast<int64_t>(view.region.y)*pixels->height/layout_.height);
      const int sw = static_cast<int>(static_cast<int64_t>(view.region.width)*pixels->width/layout_.width);
      const int sh = static_cast<int>(static_cast<int64_t>(view.region.height)*pixels->height/layout_.height);
      StretchDIBits(dc, 0, 0, client.right, client.bottom, sx, sy, sw, sh, pixels->bgra.data(), &info, DIB_RGB_COLORS, SRCCOPY);
    }
    if (view.menu && view.toolbar && view.handle_rect.right > view.handle_rect.left) {
      DrawToolbarHandle(dc, view.handle_rect);
    }
    BitBlt(target, 0, 0, client.right, client.bottom, dc, 0, 0, SRCCOPY);
    EndPaint(view.window, &paint);
  }
  static LRESULT CALLBACK WindowProc(HWND window, UINT message, WPARAM wp, LPARAM lp) {
    auto* view = reinterpret_cast<View*>(GetWindowLongPtrW(window, GWLP_USERDATA));
    if (message == WM_NCCREATE) {
      view = static_cast<View*>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams);
      view->window = window; SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(view));
    }
    if (!view) return DefWindowProcW(window, message, wp, lp);
    auto& owner = *view->owner;
    switch (message) {
      case WM_PAINT: owner.Paint(*view); return 0;
      case WM_DRAWITEM: DrawToolbarButton(*reinterpret_cast<DRAWITEMSTRUCT*>(lp)); return TRUE;
      case WM_SIZE: owner.LayoutToolbar(*view); return 0;
      case WM_DPICHANGED: owner.LayoutToolbar(*view); owner.ScheduleRedetect(); return 0;
      case WM_SETTINGCHANGE: owner.ScheduleRedetect(); return 0;
      case WM_COMMAND:
        if (HIWORD(wp) == BN_CLICKED && LOWORD(wp) >= 101 && LOWORD(wp) <= 103) {
          owner.ReleaseInput();
          PostMessageW(window, LOWORD(wp) == 101 ? WM_APP+8 : WM_APP+7, LOWORD(wp) - 100, 0);
        }
        return 0;
      case WM_ERASEBKGND: return 1;
      case WM_TIMER: {
        if (wp == 2) {
          // Debounced monitor change: re-detect and rebuild instead of ending.
          // The rebuild may destroy this window, so touch nothing afterwards.
          KillTimer(window, 2);
          owner.redetect_pending_ = false;
          owner.Redetect();
          return 0;
        }
        uint64_t sequence = 0; { std::lock_guard<std::mutex> lock(owner.feed_->mutex); if (owner.feed_->latest) sequence = owner.feed_->latest->sequence; }
        POINT point{}; GetCursorPos(&point); ScreenToClient(window, &point);
        const bool menu = view->toolbar && (owner.toolbar_dragging_ || PtInRect(&view->toolbar_rect, point));
        if (menu != view->menu) {
          for (HWND button : view->buttons) if (button) ShowWindow(button, menu ? SW_SHOWNOACTIVATE : SW_HIDE);
        }
        if (sequence != view->sequence || menu != view->menu) { view->sequence = sequence; view->menu = menu; InvalidateRect(window, nullptr, FALSE); }
        if (view->toolbar && ++owner.slow_ticks_ >= 63) {
          // Monitor arrangement changes emit no message; diff about once a second.
          owner.slow_ticks_ = 0;
          owner.CheckMonitorSnapshot();
        }
        return 0;
      }
      case WM_APP+8: case WM_CLOSE: owner.Close(); return 0;
      case WM_DISPLAYCHANGE: AdNativeLog("info", "WM_DISPLAYCHANGE received"); owner.ScheduleRedetect(); return 0;
      case WM_APP+7: owner.Open(wp == 3, window); return 0;
      case WM_SETFOCUS: owner.SyncKeyboard(); return 0;
      case WM_KILLFOCUS: owner.ReleaseInput(); return 0;
      case WM_INPUTLANGCHANGE: owner.SyncKeyboard(); return DefWindowProcW(window, message, wp, lp);
      case WM_KEYDOWN: case WM_SYSKEYDOWN: case WM_KEYUP: case WM_SYSKEYUP: {
        const bool down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        if (down && wp == VK_ESCAPE && (GetKeyState(VK_CONTROL)&0x8000) && (GetKeyState(VK_MENU)&0x8000)) { owner.Close(); return 0; }
        // Windows emits a synthetic left Ctrl alongside AltGr; Linux needs only Right Alt.
        if (wp == VK_CONTROL && !(lp & (1LL<<24))) {
          MSG next{};
          if (PeekMessageW(&next, window, WM_KEYFIRST, WM_KEYLAST, PM_NOREMOVE) && next.wParam == VK_MENU && (next.lParam & (1LL<<24)) && next.time == static_cast<DWORD>(GetMessageTime())) return 0;
        }
        owner.SyncKeyboard();
        const int code = ad_desktop::EvdevScan(static_cast<unsigned>((lp>>16)&0xff), (lp & (1LL<<24)) != 0, static_cast<unsigned>(wp));
        if (down) {
          // Ignore OS repeat, including repeat arriving after focus released a key.
          if (!(lp & (1LL<<30)) && code && !owner.keys_.count(code) && owner.Key(code, true)) owner.keys_.insert(code);
        } else if (owner.keys_.count(code) && owner.Key(code, false)) owner.keys_.erase(code);
        return 0;
      }
      case WM_MOUSEMOVE: {
        if (owner.toolbar_dragging_) { owner.DragToolbar(*view, GET_X_LPARAM(lp), GET_Y_LPARAM(lp)); return 0; }
        const POINT point{GET_X_LPARAM(lp), GET_Y_LPARAM(lp)};
        if (view->toolbar && PtInRect(&view->toolbar_rect, point)) return 0;
        owner.Move(*view, point.x, point.y);
        return 0;
      }
      case WM_LBUTTONDOWN: case WM_RBUTTONDOWN: case WM_MBUTTONDOWN: case WM_XBUTTONDOWN:
      case WM_LBUTTONUP: case WM_RBUTTONUP: case WM_MBUTTONUP: case WM_XBUTTONUP: {
        const bool down = message == WM_LBUTTONDOWN || message == WM_RBUTTONDOWN || message == WM_MBUTTONDOWN || message == WM_XBUTTONDOWN;
        const POINT point{GET_X_LPARAM(lp), GET_Y_LPARAM(lp)};
        if (down && message == WM_LBUTTONDOWN && view->toolbar && PtInRect(&view->handle_rect, point)) {
          owner.BeginToolbarDrag(*view, point.x, point.y);
          return 0;
        }
        if (owner.toolbar_dragging_) {
          if (!down && message == WM_LBUTTONUP) owner.EndToolbarDrag(*view);
          return 0;
        }
        // A remote drag may end over the local toolbar: always release it.
        if (down && view->toolbar && PtInRect(&view->toolbar_rect, point)) return 0;
        SetFocus(window); owner.Move(*view, GET_X_LPARAM(lp), GET_Y_LPARAM(lp));
        const int button = message == WM_LBUTTONDOWN || message == WM_LBUTTONUP ? 1 : message == WM_RBUTTONDOWN || message == WM_RBUTTONUP ? 2 : message == WM_MBUTTONDOWN || message == WM_MBUTTONUP ? 3 : GET_XBUTTON_WPARAM(wp) == XBUTTON1 ? 4 : 5;
        if (owner.Button(button, down)) {
          if (down) { owner.buttons_.insert(button); SetCapture(window); }
          else owner.buttons_.erase(button);
        }
        if (!down && owner.buttons_.empty()) { ReleaseCapture(); }
        return 0;
      }
      case WM_CAPTURECHANGED:
        owner.toolbar_dragging_ = false;
        if (reinterpret_cast<HWND>(lp) != window) owner.ReleaseInput();
        return 0;
      case WM_MOUSEWHEEL: case WM_MOUSEHWHEEL: {
        const int delta = GET_WHEEL_DELTA_WPARAM(wp);
        owner.Send("{\"type\":\"input\",\"event\":{\"PointerWheel\":{\"horizontal\":" + std::to_string(message == WM_MOUSEHWHEEL ? delta : 0) + ",\"vertical\":" + std::to_string(message == WM_MOUSEWHEEL ? -delta : 0) + "}}}"); return 0;
      }
      default: return DefWindowProcW(window, message, wp, lp);
    }
  }
  std::shared_ptr<Feed> feed_;
  HWND parent_;
  RemoteCommand command_;
  ad_desktop::Layout layout_;
  std::vector<std::unique_ptr<View>> views_;
  std::set<int> keys_, buttons_;
  long pointer_x_ = 0, pointer_y_ = 0;
  uint32_t keyboard_id_ = 0;
  uint32_t tab_width_ = 0, tab_height_ = 0;
  bool tab_size_known_ = false;
  bool configured_ = false;
  bool all_monitors_ = false;
  HMONITOR parent_monitor_ = nullptr;
  bool follow_parent_ = false;
  bool redetect_pending_ = false;
  uint32_t slow_ticks_ = 0;
  std::vector<RECT> monitor_snapshot_;
  ToolbarEdge toolbar_edge_ = ToolbarEdge::Top;
  bool toolbar_floating_ = false;
  POINT toolbar_position_{0, 0};
  bool toolbar_dragging_ = false;
  POINT toolbar_drag_offset_{0, 0};
};
