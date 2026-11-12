#!/bin/sh
# Runs as an unprivileged user in the agent build container, with /tmp mode 0755.
set -eu
agent="$1"
test "$(id -u)" != 0
test ! -w /tmp
unset XDG_RUNTIME_DIR
export TMPDIR=/tmp
terminal=00000000-0000-4000-8000-000000000001
"$agent" terminal route "$terminal" 00000000-0000-4000-8000-000000000002 436865636b
test -s "$HOME/.cache/newport/run/terminals/routes/00000000000040008000000000000001.json"
if "$agent" open google.com > "$HOME/open.log" 2>&1; then
    echo 'Invalid URL unexpectedly accepted' >&2
    exit 1
fi
grep -Fq 'Invalid browser URL' "$HOME/open.log"
"$agent" runtime --stdio < /dev/null > "$HOME/hello"
# Inspect the endpoint bytes without interpreting protobuf framing as text.
LC_ALL=C grep -aFq "$HOME/.cache/newport/run/" "$HOME/hello"
"$agent" display --backend wayland -- sh -c 'test -S "$WAYLAND_DISPLAY"'
# Optional X11 failure must not prevent Browser/CLI/Wayland session startup.
"$agent" serve fixture --browser --clipboard < /dev/null > "$HOME/session.stdout" 2> "$HOME/session.log"
test ! -s "$HOME/session.stdout"
grep -q 'X11 clipboard unavailable' "$HOME/session.log"
test ! -e /tmp/.X11-unix
echo 'PASS: Needle services start without writable global /tmp'
