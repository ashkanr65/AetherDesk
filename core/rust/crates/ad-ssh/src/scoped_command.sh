#!/usr/bin/env bash
# Owned AI/Tools job. stdin is a lease, never user-command input. No PTY and
# no global pkill: signals target only the new session created for this job.
set +m
umask 077
command -v setsid >/dev/null || { echo 'Scoped commands require setsid on the SSH host' >&2; exit 126; }
jobdir=$(mktemp -d "${TMPDIR:-/tmp}/aetherdesk-job.XXXXXXXX") || exit 126
launcher=
group=
cleanup() {
    trap '' HUP INT TERM
    if [[ -n "$launcher" ]]; then
        # Cancellation can race the group leader's first instruction. Wait
        # for its identity before signalling; never assume $! is the PGID.
        for ((i=0; i<100; i++)); do
            if [[ -s "$jobdir/group" ]]; then
                read -r group < "$jobdir/group"
                break
            fi
            kill -0 "$launcher" 2>/dev/null || break
            sleep 0.02
        done
        if [[ "$group" =~ ^[0-9]+$ ]] && ((group > 1)); then
            kill -TERM -- "-$group" 2>/dev/null || :
            sleep 0.2
            kill -KILL -- "-$group" 2>/dev/null || :
        else
            kill -TERM "$launcher" 2>/dev/null || :
        fi
        wait "$launcher" 2>/dev/null || :
    fi
    rm -rf -- "$jobdir"
}
trap cleanup EXIT
trap 'exit 125' HUP INT TERM
read -r -t 10 gate || exit 125
[[ "$gate" == AETHER_RUN ]] || exit 125
setsid bash -c '
    set +m
    jobdir=$1
    # A caught signal is reset for exec children; ignoring TERM here would
    # accidentally make the user command ignore it too.
    trap ":" TERM
    printf "%s\n" "$$" > "$jobdir/group"
    deadline=$((SECONDS + 10))
    until [[ -f "$jobdir/start" ]]; do
        ((SECONDS < deadline)) || exit 125
        sleep 0.02
    done
    bash -lc "$2" </dev/null &
    work=$!
    wait "$work"
    printf "%s\n" "$?" > "$jobdir/status"
    # Keep the session leader alive until cleanup, preventing PGID reuse and
    # allowing cleanup of children left behind by a successful command.
    # Hide only idle-leader signal diagnostics, preserving command stderr.
    exec 2>/dev/null
    while :; do sleep 1; done
' aetherdesk-job "$jobdir" "$1" &
launcher=$!
deadline=$((SECONDS + 10))
until [[ -s "$jobdir/group" ]]; do
    kill -0 "$launcher" 2>/dev/null || exit 126
    ((SECONDS < deadline)) || exit 126
    sleep 0.02
done
read -r group < "$jobdir/group"
[[ "$group" =~ ^[0-9]+$ ]] && ((group > 1)) || exit 126
touch "$jobdir/start" || exit 126
deadline=$((SECONDS + 10))
pending=
while :; do
    if [[ -s "$jobdir/status" ]]; then
        read -r status < "$jobdir/status"
        [[ "$status" =~ ^[0-9]+$ ]] && ((status <= 255)) || exit 126
        exit "$status"
    fi
    # -t 0 only probes readiness and consumes nothing. Bash 4.4 timed reads
    # can lose consumed input at the alarm boundary, even with -N 1. Never
    # put a timer around the read that actually consumes a heartbeat byte.
    if IFS= read -r -t 0; then
        fragment=
        # We are stdin's only reader, so its ready byte cannot disappear.
        # The C locale prevents waiting for the rest of a UTF-8 character;
        # a NUL delimiter prevents Bash from silently skipping a lone NUL.
        LC_ALL=C IFS= read -r -n 1 -d '' fragment || exit 125
        [[ -n "$fragment" ]] || exit 125
        if [[ "$fragment" == $'\n' ]]; then
            [[ "$pending" == AETHER_PULSE ]] || exit 125
            pending=
            deadline=$((SECONDS + 10))
        else
            pending+=$fragment
            ((${#pending} <= 64)) || exit 125
        fi
    else
        sleep 0.05
    fi
    # EOF is readable and fails the byte read above. A silent transport with
    # stdin still open must also expire when the client stops renewing.
    ((SECONDS < deadline)) || exit 124
done
