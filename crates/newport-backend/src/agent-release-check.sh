set -eu
uname -s
uname -m
agent="$HOME/.local/bin/newport-agent"
if test -x "$agent"; then
    hash="$(sha256sum "$agent")"
    hash=${hash%% *}
    case "$hash" in ''|*[!a-f0-9]*) exit 1 ;; esac
    test "${#hash}" = 64
    printf '%s\n' "$hash"
    metadata="$HOME/.local/share/newport/needle/builds/$hash/needle-release.json"
    if test -e "$metadata" || test -L "$metadata"; then
        test -f "$metadata" && ! test -L "$metadata" || { echo 'Invalid Needle release file.' >&2; exit 1; }
        # One byte beyond the envelope limit lets the verifier reject truncation.
        head -c 8193 "$metadata"
    fi
elif test -e "$agent" || test -L "$agent"; then
    echo 'Needle exists but is not executable.' >&2
    exit 1
else
    printf 'missing\n'
fi
