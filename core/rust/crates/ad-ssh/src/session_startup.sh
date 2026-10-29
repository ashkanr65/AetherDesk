# Runs only an explicitly enabled, per-connection script. Never edit shell
# profiles or shared server configuration. Temporary state is private to its run.
set -u
mode=$1
shift
umask 077
base="$HOME/.aetherdesk/startup"
if [ -L "$base" ]; then echo 'Startup directory cannot be a symbolic link' >&2; exit 1; fi
mkdir -p -- "$base" || exit 1
stage=$(mktemp -d "$base/run.XXXXXXXXXXXX") || exit 1
script="$stage/commands.sh"
environment="$stage/environment.sh"
printf '%s\n' "${AETHERDESK_STARTUP_COMMANDS-}" > "$script" || exit 1
limit=${AETHERDESK_STARTUP_TIMEOUT:-30}
keep_going=${AETHERDESK_STARTUP_CONTINUE:-0}
unset AETHERDESK_STARTUP_COMMANDS AETHERDESK_STARTUP_TIMEOUT AETHERDESK_STARTUP_CONTINUE
timeout --kill-after=2 "$limit" bash -c '
    set -a
    source "$1"
    result=$?
    if [ "$result" -ne 0 ]; then exit "$result"; fi
    export -p > "$2" || exit 1
    printf "cd -- %q\n" "$PWD" >> "$2"
' aetherdesk-startup "$script" "$environment"
result=$?
if [ "$result" -eq 0 ] && [ ! -f "$environment" ]; then result=1; fi
rm -f -- "$script"
if [ "$result" -ne 0 ]; then
    printf 'Startup commands failed (exit %s)\n' "$result" >&2
    if [ "$keep_going" != 1 ]; then
        rm -f -- "$environment"
        rmdir -- "$stage"
        exit "$result"
    fi
    : > "$environment"
fi
if [ "$mode" = prepare ]; then
    printf '\nAETHERDESK_STARTUP_ENV_V1=%s\n' "$environment"
elif [ "$mode" = exec ]; then
    source "$environment"
    /bin/rm -f -- "$environment"
    /bin/rmdir -- "$stage"
    exec "$@"
else
    rm -f -- "$environment"
    rmdir -- "$stage"
    exit 2
fi
