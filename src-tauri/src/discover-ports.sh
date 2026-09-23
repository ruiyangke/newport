listeners=$(ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null) || { echo 'Port discovery requires ss or netstat on the server.' >&2; exit 1; }
# Reuse existing non-interactive sudo permission; never prompt or alter sudoers.
if elevated=$(sudo -n ss -tlnp 2>/dev/null || sudo -n netstat -tlnp 2>/dev/null); then
    listeners="$listeners
$elevated"
fi
printf '%s\n' "$listeners"
printf '__PORTHOP_DOCKER_PORTS__\n'
docker ps --format '{"name":{{json .Names}},"ports":{{json .Ports}}}' 2>/dev/null || true
printf '__PORTHOP_PROCESS_DETAILS__\n'
# Encode fields so paths, whitespace and NUL-delimited argv cannot corrupt records.
hex() { od -An -v -tx1 | tr -d ' \n'; }
start_time() { sed 's/.*) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }
printf '%s\n' "$listeners" | sed -n -e 's/.*pid=\([0-9][0-9]*\).*/\1/p' -e 's/.*LISTEN[[:space:]]*\([0-9][0-9]*\)\/.*/\1/p' | sort -nu | head -n 256 | while read -r pid; do
    before=$(start_time "$pid")
    [ -n "$before" ] || continue
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null | head -c 4096 | hex)
    cwd=$(readlink "/proc/$pid/cwd" 2>/dev/null | head -c 4096 | hex)
    args=$(head -c 4096 "/proc/$pid/cmdline" 2>/dev/null | hex)
    user=$(ps -p "$pid" -o user= 2>/dev/null | head -c 256 | hex)
    [ "$before" = "$(start_time "$pid")" ] || continue
    printf '%s|%s|%s|%s|%s\n' "$pid" "$exe" "$cwd" "$args" "$user"
done
