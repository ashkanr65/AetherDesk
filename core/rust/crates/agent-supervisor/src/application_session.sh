#!/usr/bin/env bash
set -eu
umask 077
unset WAYLAND_DISPLAY
export GDK_BACKEND=x11 QT_QPA_PLATFORM=xcb XDG_SESSION_TYPE=x11
session_runtime=${XDG_RUNTIME_DIR:?}
environment_file="$session_runtime/application-environment.sh"
launch_pipe="$session_runtime/application-launch.fifo"
for stale in "$launch_pipe" "$environment_file"; do
    if [ -e "$stale" ] || [ -L "$stale" ]; then
        test ! -L "$stale" && test -O "$stale"
        if [ "$stale" = "$launch_pipe" ]; then test -p "$stale"; else test -f "$stale"; fi
        rm -f -- "$stale"
    fi
done
mkfifo -m 600 -- "$launch_pipe"
# Publish only a complete snapshot. It contains exports and cwd from the
# one-time startup commands, plus this session's own D-Bus identity.
environment_temporary=$(mktemp "$session_runtime/application-environment.XXXXXXXX")
{
    export -p
    printf 'cd -- %q\n' "$PWD"
} > "$environment_temporary"
mv -f -- "$environment_temporary" "$environment_file"
# Children stay in the compositor-owned process group. Disconnect preserves
# this launcher; explicit End session stops the WM and every launched child.
exec 9<>"$launch_pipe"
xfwm4 --compositor=on &
window_manager=$!
(
    while IFS= read -r command <&9; do
        bash --noprofile --norc -c "$command" </dev/null >/dev/null &
    done
) &
wait "$window_manager"
