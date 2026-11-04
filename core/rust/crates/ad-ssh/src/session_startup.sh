# Runs only an explicitly enabled, per-connection script. Never edit shell
# profiles or shared server configuration. Temporary state is private to its run.
set -u
mode=$1
shift
umask 077
base="${AETHERDESK_TEMP_DIR:-$HOME/.aetherdesk}/startup"
if [ -L "$base" ]; then echo 'Startup directory cannot be a symbolic link' >&2; exit 1; fi
mkdir -p -- "$base" || exit 1
__ad_startup_stage=$(mktemp -d "$base/run.XXXXXXXXXXXX") || exit 1
__ad_startup_script="$__ad_startup_stage/commands.sh"
__ad_startup_environment="$__ad_startup_stage/environment.sh"
readonly __ad_startup_stage __ad_startup_script __ad_startup_environment
cleanup_startup() {
    /bin/rm -f -- "$__ad_startup_script" "$__ad_startup_environment"
    /bin/rmdir -- "$__ad_startup_stage" 2>/dev/null || true
}
trap cleanup_startup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
printf '%s\n' "${AETHERDESK_STARTUP_COMMANDS-}" > "$__ad_startup_script" || exit 1
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
' aetherdesk-startup "$__ad_startup_script" "$__ad_startup_environment"
result=$?
if [ "$result" -eq 0 ] && [ ! -f "$__ad_startup_environment" ]; then result=1; fi
rm -f -- "$__ad_startup_script"
if [ "$result" -ne 0 ]; then
    printf 'Startup commands failed (exit %s)\n' "$result" >&2
    if [ "$keep_going" != 1 ]; then
        rm -f -- "$__ad_startup_environment"
        rmdir -- "$__ad_startup_stage"
        exit "$result"
    fi
    : > "$__ad_startup_environment"
fi
if [ "$mode" = prepare ]; then
    # Ownership is handed to the interactive shell/connection lease.
    trap - EXIT HUP INT TERM
    printf '\nAETHERDESK_STARTUP_ENV_V1=%s\n' "$__ad_startup_environment"
elif [ "$mode" = exec ]; then
    source "$__ad_startup_environment"
    /bin/rm -f -- "$__ad_startup_environment"
    /bin/rmdir -- "$__ad_startup_stage"
    trap - EXIT HUP INT TERM
    exec "$@"
else
    rm -f -- "$__ad_startup_environment"
    rmdir -- "$__ad_startup_stage"
    exit 2
fi
