#!/usr/bin/env bash
set -Eeuo pipefail
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
for tool in podman nft aardvark-dns pkg-config gcc g++ make cmake ninja patch python3 runuser unshare; do
  if ! command -v "$tool" >/dev/null && [[ ! -x "/usr/libexec/podman/$tool" && ! -x "/usr/lib/podman/$tool" ]]; then
    echo "Missing Ubuntu build tool: $tool" >&2
    exit 1
  fi
done
test -x /opt/aetherdesk-xwayland/bin/Xwayland
id ashkan
runuser -u ashkan -- /home/ashkan/.cargo/bin/cargo --version
runuser -u ashkan -- /home/ashkan/.cargo/bin/rustup component list --installed
pkg-config --modversion wayland-server xkbcommon gstreamer-1.0 gstreamer-app-1.0 gstreamer-video-1.0 libinput libudev gbm libdrm egl glesv2 pixman-1
audit=$(dpkg --audit)
[[ -z "$audit" ]] || { printf '%s\n' "$audit" >&2; exit 1; }
# Existing Ubuntu only; no installation, global mount change, or WSL shutdown.
echo 'Existing Ubuntu and Linux build tools checked.'
