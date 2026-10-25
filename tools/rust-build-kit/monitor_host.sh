#!/bin/sh
# Non-PTY, read-only Linux sampling. No terminal commands or persistent daemon.
export LC_ALL=C
proc=${AETHER_MONITOR_PROC_ROOT:-/proc}
printf 'AETHER_MONITOR_V1\n'
case "$1" in
fast)
    printf 'HOST\t'; uname -n
    awk '/^cpu / { total=0; for(i=2;i<=9;i++) total+=$i; printf "CPU\t%.0f\t%.0f\n",total,$5+$6 }' "$proc/stat"
    awk '/^MemTotal:/ {t=$2} /^MemAvailable:/ {a=$2; found=1} /^MemFree:/ {f=$2} /^Buffers:/ {b=$2} /^Cached:/ {c=$2} /^SwapTotal:/ {s=$2} /^SwapFree:/ {sf=$2} END {if(t>0) printf "MEM\t%.0f\t%.0f\t%.0f\t%.0f\n",t,(found?a:f+b+c),s,sf}' "$proc/meminfo"
    awk '{printf "UP\t%.0f\n",$1}' "$proc/uptime"
    awk -F: 'NR>2 {gsub(/ /,"",$1); if($1!="lo") {n=split($2,a,/ +/); offset=(a[1]==""?1:0); printf "NET\t%s\t%s\t%s\n",$1,a[1+offset],a[9+offset]}}' "$proc/net/dev"
    ;;
slow)
    printf 'HOME\t%s\n' "$HOME"
    printf 'USERS_BEGIN\n'
    if command -v who >/dev/null 2>&1; then
        who || printf 'ERR\tusers\tLogin records unavailable\n'
    else
        printf 'ERR\tusers\tLogin records unavailable\n'
    fi
    printf 'USERS_END\nDISKS_BEGIN\n'
    df -Pk -- "$HOME" 2>/dev/null || printf 'ERR\tdisks\tLocal filesystem statistics unavailable\n'
    printf 'DISKS_END\n'
    ;;
homes)
    # Explicit scan only; never part of periodic sampling. No sudo, symlink
    # following or filesystem crossing. The entire du process has an 8s budget.
    base=${AETHER_MONITOR_HOME_ROOT:-/home}
    set --
    count=0
    for directory in "$base"/*; do
        [ -d "$directory" ] && [ ! -L "$directory" ] || continue
        case "$directory" in *"$(printf '\t')"*) continue;; esac
        if [ "$count" -ge 64 ]; then printf 'ERR\thomes\tFolder limit reached\n'; break; fi
        owner=$(stat -c %U -- "$directory" 2>/dev/null) || owner='?'
        printf 'HOME_DIR\t%s\t%s\n' "$owner" "$directory"
        set -- "$@" "$directory"
        count=$((count+1))
    done
    if [ "$#" -gt 0 ]; then
        # Keep partial output on timeout/permission errors; it is marked below.
        { timeout 8 du -skx -- "$@" 2>/dev/null || printf 'ERR\thomes\tIncomplete scan\n'; } |
            awk 'BEGIN {FS="\t"} /^ERR\t/ {print; next} $1 ~ /^[0-9]+$/ && NF==2 {printf "HOME_SIZE\t%s\t%s\n",$1,$2}'
    fi
    ;;
*) exit 2;;
esac
