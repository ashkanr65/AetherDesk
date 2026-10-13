#!/usr/bin/env bash
set -euo pipefail
prefix="${1:-/opt/aetherdesk-xwayland}"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p /tmp/aetherdesk-xwayland-build
cd /tmp/aetherdesk-xwayland-build
fetch() {
  local name="$1" url="$2" checksum="$3"
  curl --fail --location --retry 3 "$url" --output "$name.tar.xz"
  printf '%s  %s\n' "$checksum" "$name.tar.xz" | sha256sum --check -
  tar -xf "$name.tar.xz"
}
build() {
  local name="$1"
  shift
  local options=()
  [[ -f "$name/build/meson-private/coredata.dat" ]] && options+=(--wipe)
  meson setup "${options[@]}" "$name/build" "$name" --prefix="$prefix" --libdir=lib --buildtype=release "$@"
  ninja -C "$name/build"
  ninja -C "$name/build" install
}
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig:$prefix/share/pkgconfig:${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="$prefix/lib:${LD_LIBRARY_PATH:-}"
fetch xorgproto-2024.1 https://www.x.org/releases/individual/proto/xorgproto-2024.1.tar.xz 372225fd40815b8423547f5d890c5debc72e88b91088fbfb13158c20495ccb59
build xorgproto-2024.1 -Dlegacy=true
fetch wayland-protocols-1.34 https://gitlab.freedesktop.org/wayland/wayland-protocols/-/releases/1.34/downloads/wayland-protocols-1.34.tar.xz c59b27cacd85f60baf4ee5f80df5c0d15760ead6a2432b00ab7e2e0574dcafeb
build wayland-protocols-1.34 -Dtests=false
fetch libxcvt-0.1.3 https://www.x.org/releases/individual/lib/libxcvt-0.1.3.tar.xz a929998a8767de7dfa36d6da4751cdbeef34ed630714f2f4a767b351f2442e01
build libxcvt-0.1.3
fetch libdrm-2.4.120 https://dri.freedesktop.org/libdrm/libdrm-2.4.120.tar.xz 3bf55363f76c7250946441ab51d3a6cc0ae518055c0ff017324ab76cdefb327a
build libdrm-2.4.120 -Dauto_features=disabled -Dtests=false
fetch xwayland-24.1.10 https://www.x.org/releases/individual/xserver/xwayland-24.1.10.tar.xz 459762be8ea046c94386687d77a87add6073868bee14f02913eafebb945b7aa0
patch -d xwayland-24.1.10 -p1 < "$script_dir/xwayland-monitors.patch"
# Smithay's Pixman compositor uses shared-memory buffers, without a GPU/GLX.
build xwayland-24.1.10 -Dglamor=false -Dglx=false -Ddri3=false -Ddrm=false \
  -Dxwayland_ei=false -Dlibdecor=false -Dxvfb=false -Dxselinux=false \
  -Ddocs=false -Ddevel-docs=false -Dxkb_dir=/usr/share/X11/xkb -Dxkb_bin_dir=/usr/bin
"$prefix/bin/Xwayland" -version
install -Dm644 xwayland-24.1.10/COPYING "$prefix/share/licenses/aetherdesk-xwayland/Xwayland-COPYING"
install -Dm644 libxcvt-0.1.3/COPYING "$prefix/share/licenses/aetherdesk-xwayland/libxcvt-COPYING"
install -Dm644 libdrm-2.4.120.tar.xz "$prefix/share/licenses/aetherdesk-xwayland/libdrm-source.tar.xz"
