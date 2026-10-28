# Called only by explicit installation, after candidate handshake verification.
set -eu
umask 077
dir="$HOME/.local/share/newport/needle/builds/$1"
test -d "$dir" && ! test -L "$dir"
actual="$(sha256sum "$dir/newport-agent")"
test "${actual%% *}" = "$1"
tmp="$(mktemp "$dir/.release.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
trap 'exit 1' HUP INT TERM
cat > "$tmp"
sync -f "$tmp"
# Never replace an identity recorded by another client for the same bytes.
if ! ln "$tmp" "$dir/needle-release.json" 2>/dev/null; then
    test -f "$dir/needle-release.json" && ! test -L "$dir/needle-release.json"
fi
sync -f "$dir"
