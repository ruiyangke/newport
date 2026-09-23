set -eu
umask 077
dir="$HOME/.local/bin"
mkdir -p "$dir"
if [ -e "$dir/porthop-agent" ] || [ -L "$dir/porthop-agent" ]; then
    if [ -L "$dir/porthop-agent" ] || [ ! -f "$dir/porthop-agent" ] ||
       ! grep -aEq 'porthop-agent/[123]' "$dir/porthop-agent"; then
        echo 'Cannot replace unrelated ~/.local/bin/porthop-agent.' >&2
        exit 1
    fi
fi
tmp="$(mktemp "$dir/.porthop-agent.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
trap 'exit 1' HUP INT TERM
cat > "$tmp"
actual="$(sha256sum "$tmp")"
[ "${actual%% *}" = "$1" ] || { echo 'Agent upload checksum mismatch.' >&2; exit 1; }
chmod 700 "$tmp"
[ "$("$tmp" --version)" = "porthop-agent/3" ] || exit 1
mv -f "$tmp" "$dir/porthop-agent"
"$dir/porthop-agent" install
