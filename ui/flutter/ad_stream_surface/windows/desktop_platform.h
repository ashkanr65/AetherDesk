#pragma once
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

namespace ad_desktop {
struct Rect { int x, y, width, height; };
struct Layout { int width = 0, height = 0; std::vector<Rect> monitors; };
inline Layout Arrange(const std::vector<Rect>& physical) {
  Layout layout;
  if (physical.empty() || physical.size() > 16) return layout;
  int left = physical[0].x, top = physical[0].y;
  int right = left + physical[0].width, bottom = top + physical[0].height;
  for (const auto& r : physical) {
    if (r.width <= 0 || r.height <= 0) return {};
    left = std::min(left, r.x); top = std::min(top, r.y);
    right = std::max(right, r.x + r.width); bottom = std::max(bottom, r.y + r.height);
  }
  const double width = right-left, height = bottom-top;
  const double scale = std::min({1.0, 16384.0/width, 16384.0/height, std::sqrt(33554432.0/(width*height))});
  layout.width = std::max(64, static_cast<int>(width*scale) / 2 * 2);
  layout.height = std::max(64, static_cast<int>(height*scale) / 2 * 2);
  for (const auto& r : physical) {
    const int x = static_cast<int>(std::round((r.x-left)*layout.width/width));
    const int y = static_cast<int>(std::round((r.y-top)*layout.height/height));
    const int far_x = static_cast<int>(std::round((r.x+r.width-left)*layout.width/width));
    const int far_y = static_cast<int>(std::round((r.y+r.height-top)*layout.height/height));
    layout.monitors.push_back({x, y, far_x-x, far_y-y});
  }
  return layout;
}
struct KeyboardLayout { const char* layout; const char* variant; };
inline KeyboardLayout KeyboardForId(uint32_t id) {
  // Keyboard layout IDs describe input, independently of the Windows UI language.
  switch (id) {
    case 0x00020409: return {"us", "intl"};
    case 0x00010409: return {"us", "dvorak"};
    case 0x00000809: return {"gb", ""};
    case 0x0000080a: return {"latam", ""};
    case 0x0000040a: case 0x00000c0a: return {"es", ""};
    default: break;
  }
  if (id > 0xffff) return {nullptr, nullptr};
  switch (id) {
    case 0x0409: return {"us", ""};
    case 0x0809: return {"gb", ""};
    case 0x040a: case 0x0c0a: return {"es", ""};
    case 0x080a: return {"latam", ""};
    case 0x0407: return {"de", ""};
    case 0x040c: return {"fr", ""};
    case 0x0410: return {"it", ""};
    case 0x0416: return {"br", ""};
    case 0x0816: return {"pt", ""};
    case 0x0419: return {"ru", ""};
    case 0x0429: return {"ir", ""};
    case 0x0406: return {"dk", ""};
    case 0x041d: return {"se", ""};
    case 0x0414: return {"no", ""};
    case 0x040b: return {"fi", ""};
    case 0x0415: return {"pl", ""};
    case 0x041f: return {"tr", ""};
    case 0x0405: return {"cz", ""};
    default: return {nullptr, nullptr};
  }
}
inline int EvdevScan(unsigned scan, bool extended, unsigned virtual_key) {
  if (virtual_key == 0x90) return 69; // Num Lock
  if (virtual_key == 0x13) return 119; // Pause
  if (!extended) return scan > 0 && scan <= 0x58 ? static_cast<int>(scan) : 0;
  switch (scan) {
    case 0x1c: return 96; case 0x1d: return 97; case 0x35: return 98;
    case 0x37: return 99; case 0x38: return 100; case 0x47: return 102;
    case 0x48: return 103; case 0x49: return 104; case 0x4b: return 105;
    case 0x4d: return 106; case 0x4f: return 107; case 0x50: return 108;
    case 0x51: return 109; case 0x52: return 110; case 0x53: return 111;
    case 0x5b: return 125; case 0x5c: return 126; case 0x5d: return 127;
    default: return 0;
  }
}
}  // namespace ad_desktop
