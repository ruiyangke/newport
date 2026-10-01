set -eu
umask 077
dir="$HOME/.local/bin"
mkdir -p "$dir"
if [ -e "$dir/newport-agent" ] || [ -L "$dir/newport-agent" ]; then
    if [ -L "$dir/newport-agent" ] || [ ! -f "$dir/newport-agent" ] ||
       ! grep -aEq 'newport-agent/[123456]' "$dir/newport-agent"; then
        echo 'Cannot replace unrelated ~/.local/bin/newport-agent.' >&2
        exit 1
    fi
fi
tmp="$(mktemp "$dir/.newport-agent.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
trap 'exit 1' HUP INT TERM
cat > "$tmp"
actual="$(sha256sum "$tmp")"
[ "${actual%% *}" = "$1" ] || { echo 'Agent upload checksum mismatch.' >&2; exit 1; }
chmod 700 "$tmp"
[ "$("$tmp" --version)" = "newport-agent/6" ] || exit 1
mv -f "$tmp" "$dir/newport-agent"
"$dir/newport-agent" install
