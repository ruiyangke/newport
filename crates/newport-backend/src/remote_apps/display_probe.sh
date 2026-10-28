# Read-only prerequisite check; does not start a display or inspect the desktop.
if ! command -v Xwayland >/dev/null 2>&1; then
    printf 'xwayland=missing\n'
elif ! command -v timeout >/dev/null 2>&1; then
    printf 'xwayland=unverified\n'
else
    # Kill the entire probe group at the deadline. A TERM grace period can let
    # the wrapper exit first, leaving a stuck child holding the pipe open.
    # The final marker preserves Xwayland's exit status through the output cap.
    result=$(LC_ALL=C timeout -s KILL 2 sh -c 'Xwayland -version 2>&1; printf "\nNEWPORT_XWAYLAND_STATUS=%s\n" "$?"' | head -c 8193)
    last=$(printf '%s\n' "$result" | tail -n 1)
    if [ "${#result}" -le 8192 ] && [ "$last" = 'NEWPORT_XWAYLAND_STATUS=0' ]; then
        version=$(printf '%s\n' "$result" | sed -n 's/.*Xwayland Version \([^[:space:]]*\).*/\1/p' | head -n 1)
        if [ -n "$version" ]; then
            printf 'xwayland=%s\n' "$version"
        else
            printf 'xwayland=unverified\n'
        fi
    else
        printf 'xwayland=unverified\n'
    fi
fi
