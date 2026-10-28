set -eu
uname -s
uname -m
builds="$HOME/.local/share/newport/needle/builds"
agent="$HOME/.local/bin/newport-agent"
if test -x "$agent"; then
    hash="$(sha256sum "$agent")"
    hash=${hash%% *}
    case "$hash" in ''|*[!a-f0-9]*) exit 1 ;; esac
    test "${#hash}" = 64
    printf '%s\n' "$hash"
elif test -e "$agent" || test -L "$agent"; then
    echo 'Needle exists but is not executable.' >&2
    exit 1
else
    hash=missing
    printf 'missing\n'
fi
# The builds this account's processes run, such as another desktop's runtime.
# Left out where processes can't be listed.
if test -e /proc/self/exe; then
    printf 'running'
    ls -l /proc/[0-9]*/exe 2>/dev/null | awk -v p=" -> $builds/" '{
        i = index($0, p)
        if (i == 0) next
        h = substr($0, i + length(p), 65)
        if (h ~ /^[0-9a-f]+\/$/ && length(h) == 65) printf " %s", substr(h, 1, 64)
    }' || true
    printf '\n'
fi
# The builds ready to run.
printf 'staged'
for build in "$builds"/*; do
    if test -x "$build/newport-agent"; then
        printf ' %s' "${build##*/}"
    fi
done
printf '\n'
if test "$hash" != missing; then
    metadata="$builds/$hash/needle-release.json"
    if test -e "$metadata" || test -L "$metadata"; then
        test -f "$metadata" && ! test -L "$metadata" || { echo 'Invalid Needle release file.' >&2; exit 1; }
        # One byte beyond the envelope limit lets the verifier reject truncation.
        head -c 8193 "$metadata"
    fi
fi
