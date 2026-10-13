#include "desktop_platform.h"
#ifdef NDEBUG
#undef NDEBUG
#endif
#include <cassert>

int main() {
  using namespace ad_desktop;
  auto one = Arrange({{0,0,1920,1080}});
  assert(one.width==1920 && one.height==1080 && one.monitors.size()==1);
  auto three = Arrange({{0,0,3840,2160},{-3840,0,3840,2160},{3840,0,3840,2160}});
  assert(three.width==11520 && three.height==2160 && three.monitors.size()==3);
  assert(three.monitors[0].x==3840 && three.monitors[1].x==0 && three.monitors[2].x==7680);
  auto stacked = Arrange({{0,0,1920,1080},{200,-1440,2560,1440}});
  assert(stacked.width==2760 && stacked.height==2520 && stacked.monitors[0].y==1440);
  // Physical pixels must survive a 200% laptop / 100% external-display pairing.
  for (int offset : {0, 360, 720}) {
    auto mixed = Arrange({{0,0,2880,1800},{2880,offset,1920,1080}});
    assert(mixed.width==4800 && mixed.height==1800 && mixed.monitors.size()==2);
    assert(mixed.monitors[0].width==2880 && mixed.monitors[0].height==1800);
    assert(mixed.monitors[1].width==1920 && mixed.monitors[1].height==1080);
    assert(mixed.monitors[1].x==2880 && mixed.monitors[1].y==offset);
  }
  auto externalLeft = Arrange({{0,0,2880,1800},{-1920,720,1920,1080}});
  assert(externalLeft.width==4800 && externalLeft.height==1800);
  assert(externalLeft.monitors[0].x==1920 && externalLeft.monitors[1].x==0);
  auto bounded = Arrange({{0,0,7680,4320},{7680,0,7680,4320},{15360,0,7680,4320}});
  assert(bounded.width<=16384 && int64_t(bounded.width)*bounded.height<=33554432);
  for (auto m:bounded.monitors) assert(m.x>=0 && m.y>=0 && m.x+m.width<=bounded.width && m.y+m.height<=bounded.height);
  assert(Arrange({}).monitors.empty());
  assert(std::string(KeyboardForId(0x040a).layout)=="es");
  assert(std::string(KeyboardForId(0x080a).layout)=="latam");
  assert(std::string(KeyboardForId(0x20409).variant)=="intl");
  assert(KeyboardForId(0xdeadbeef).layout==nullptr);
  assert(KeyboardForId(0xdead0409).layout==nullptr);
  assert(EvdevScan(0x38,true,0x12)==100);
  assert(EvdevScan(0x1c,true,0x0d)==96);
  assert(EvdevScan(0x45,true,0x90)==69);
  assert(EvdevScan(0x56,false,0xe2)==86);
}
