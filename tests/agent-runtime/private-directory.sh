#!/bin/sh
# Runs as an unprivileged user in the agent build container, with /tmp mode 0755.
set -eu
agent="$1"
test "$(id -u)" != 0
test ! -w /tmp
unset XDG_RUNTIME_DIR
export TMPDIR=/tmp
"$agent" sessions
if "$agent" open google.com > "$HOME/open.log" 2>&1; then
    echo 'Invalid URL unexpectedly accepted' >&2
    exit 1
fi
grep -q 'only HTTP and HTTPS' "$HOME/open.log"
test -f "$HOME/.local/state/newport/usage/lease"
"$agent" runtime --stdio < /dev/null > "$HOME/hello"
# Skip the 4-byte length: a byte above 0x7f there is invalid UTF-8 to BusyBox grep.
tail -c +5 "$HOME/hello" | grep -q "$HOME/.cache/newport/run/"
"$agent" display --backend wayland -- sh -c 'test -S "$WAYLAND_DISPLAY"'
# Optional X11 failure must not prevent Browser/CLI/Wayland session startup.
"$agent" serve fixture --browser --clipboard < /dev/null > "$HOME/ready" 2> "$HOME/session.log"
test -s "$HOME/ready"
grep -q 'X11 clipboard unavailable' "$HOME/session.log"
test ! -e /tmp/.X11-unix
echo 'PASS: Needle services start without writable global /tmp'
