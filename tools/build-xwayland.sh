#!/usr/bin/env bash
set -euo pipefail
fetch() {
  local name="$1" checksum="$2"
  shift 2
  local url round archive="$name.tar.xz" partial="$name.tar.xz.part"
  if [[ ! "$name" =~ ^[a-zA-Z0-9][a-zA-Z0-9._+-]*$ || ! "$checksum" =~ ^[a-fA-F0-9]{64}$ || "$#" -eq 0 ]]; then
    echo "Invalid pinned source download: $name" >&2
    return 1
  fi
  for url in "$@"; do
    [[ "$url" == https://* ]] || { echo "Source downloads require HTTPS: $url" >&2; return 1; }
  done
  if [[ -f "$archive" ]] && printf '%s  %s\n' "$checksum" "$archive" | sha256sum --check --status -; then
    echo "Using verified source archive: $archive"
    tar -xf "$archive"
    return
  fi
  rm -f -- "$archive" "$partial"
  # AlmaLinux 8 curl predates --retry-all-errors. Retry explicitly, including
  # TLS error 35, and try an independent mirror before retrying a failed host.
  for round in 1 2 3; do
    for url in "$@"; do
      echo "Downloading $name (round $round/3): $url"
      if curl --disable --fail --location --proto '=https' --proto-redir '=https' \
          --connect-timeout 15 --max-time 240 --speed-limit 1024 --speed-time 30 \
          "$url" --output "$partial"; then
        if printf '%s  %s\n' "$checksum" "$partial" | sha256sum --check --status -; then
          mv -f -- "$partial" "$archive"
          tar -xf "$archive"
          return
        fi
        echo "Checksum mismatch for $name from $url; discarding the download." >&2
      else
        echo "Download failed for $name from $url; trying another source." >&2
      fi
      rm -f -- "$partial"
    done
    if [[ "$round" -lt 3 ]]; then sleep "$((round * 2))"; fi
  done
  echo "Unable to download verified $name after trying every HTTPS source three times. Check network access and retry the build." >&2
  return 1
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
main() {
local prefix="${1:-/opt/aetherdesk-xwayland}"
local script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p /tmp/aetherdesk-xwayland-build
cd /tmp/aetherdesk-xwayland-build
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig:$prefix/share/pkgconfig:${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="$prefix/lib:${LD_LIBRARY_PATH:-}"
fetch xorgproto-2024.1 372225fd40815b8423547f5d890c5debc72e88b91088fbfb13158c20495ccb59 \
  https://www.x.org/releases/individual/proto/xorgproto-2024.1.tar.xz \
  https://ftp.osuosl.org/pub/blfs/conglomeration/Xorg/xorgproto-2024.1.tar.xz
build xorgproto-2024.1 -Dlegacy=true
fetch wayland-protocols-1.34 c59b27cacd85f60baf4ee5f80df5c0d15760ead6a2432b00ab7e2e0574dcafeb \
  https://gitlab.freedesktop.org/wayland/wayland-protocols/-/releases/1.34/downloads/wayland-protocols-1.34.tar.xz \
  https://ftp.osuosl.org/pub/blfs/conglomeration/wayland-protocols/wayland-protocols-1.34.tar.xz
build wayland-protocols-1.34 -Dtests=false
fetch libxcvt-0.1.3 a929998a8767de7dfa36d6da4751cdbeef34ed630714f2f4a767b351f2442e01 \
  https://www.x.org/releases/individual/lib/libxcvt-0.1.3.tar.xz \
  https://ftp.osuosl.org/pub/blfs/conglomeration/Xorg/libxcvt-0.1.3.tar.xz
build libxcvt-0.1.3
fetch libdrm-2.4.120 3bf55363f76c7250946441ab51d3a6cc0ae518055c0ff017324ab76cdefb327a \
  https://dri.freedesktop.org/libdrm/libdrm-2.4.120.tar.xz \
  https://ftp.osuosl.org/pub/blfs/conglomeration/libdrm/libdrm-2.4.120.tar.xz
build libdrm-2.4.120 -Dauto_features=disabled -Dtests=false
fetch xwayland-24.1.10 459762be8ea046c94386687d77a87add6073868bee14f02913eafebb945b7aa0 \
  https://www.x.org/releases/individual/xserver/xwayland-24.1.10.tar.xz \
  https://ftp.osuosl.org/pub/blfs/conglomeration/xwayland/xwayland-24.1.10.tar.xz
patch -d xwayland-24.1.10 -p1 < "$script_dir/xwayland-monitors.patch"
# Smithay's Pixman compositor uses shared-memory buffers, without a GPU/GLX.
build xwayland-24.1.10 -Dglamor=false -Dglx=false -Ddri3=false -Ddrm=false \
  -Dxwayland_ei=false -Dlibdecor=false -Dxvfb=false -Dxselinux=false \
  -Ddocs=false -Ddevel-docs=false -Dxkb_dir=/usr/share/X11/xkb -Dxkb_bin_dir=/usr/bin
"$prefix/bin/Xwayland" -version
install -Dm644 xwayland-24.1.10/COPYING "$prefix/share/licenses/aetherdesk-xwayland/Xwayland-COPYING"
install -Dm644 libxcvt-0.1.3/COPYING "$prefix/share/licenses/aetherdesk-xwayland/libxcvt-COPYING"
install -Dm644 libdrm-2.4.120.tar.xz "$prefix/share/licenses/aetherdesk-xwayland/libdrm-source.tar.xz"
}
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then main "$@"; fi
