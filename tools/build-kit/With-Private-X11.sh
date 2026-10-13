#!/usr/bin/env bash
set -Eeuo pipefail
export PATH="/usr/sbin:/sbin:$PATH"
[[ $(id -u) == 0 ]] || { echo 'The private test mount must be prepared by WSL root.' >&2; exit 64; }

if [[ ${1:-} != --inside ]]; then
  [[ $# -gt 0 ]] || { echo 'A test command is required.' >&2; exit 64; }
  parent_namespace=$(readlink /proc/self/ns/mnt)
  exec unshare --mount --fork --propagation private \
    bash "$(readlink -f -- "$0")" --inside "$parent_namespace" "$@"
fi

parent_namespace=${2:?missing parent mount namespace}
shift 2
[[ $(readlink /proc/self/ns/mnt) != "$parent_namespace" ]] || {
  echo 'Refusing to replace X11 sockets outside a private mount namespace.' >&2
  exit 64
}
[[ -d /tmp/.X11-unix ]] || { echo 'Ubuntu X11 socket directory is missing.' >&2; exit 69; }

# This mount exists only for the test process tree. WSLg's read-only mount,
# its X0 socket, and all other Ubuntu applications keep their original view.
mount -t tmpfs -o mode=1777,size=16m,nosuid,nodev aetherdesk-test-x11 /tmp/.X11-unix
echo 'Linux tests have a private, writable X11 socket directory.'
exec runuser -u ashkan -- env -u DISPLAY -u WAYLAND_DISPLAY \
  -u XAUTHORITY -u DBUS_SESSION_BUS_ADDRESS "$@"
