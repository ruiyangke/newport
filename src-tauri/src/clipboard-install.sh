set -eu
umask 077
for tool in bash tar flock stat date mktemp; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Clipboard sync requires $tool on the server." >&2; exit 1; }
done
dir="$HOME/.local/bin"
mkdir -p "$dir"
if [ -e "$dir/xclip" ] || [ -L "$dir/xclip" ]; then
    if { [ ! -L "$dir/xclip" ] || [ "$(readlink "$dir/xclip")" != porthop-clip ]; } && { [ ! -f "$dir/xclip" ] || [ ! -x "$dir/xclip" ]; }; then
        echo 'Cannot install: ~/.local/bin/xclip already exists. Move it aside first.' >&2
        exit 1
    fi
fi
if [ -e "$dir/porthop-clip" ] || [ -L "$dir/porthop-clip" ]; then
    if [ -L "$dir/porthop-clip" ] || [ ! -f "$dir/porthop-clip" ] || ! grep -qE '^# Porthop clipboard helper|PORTHOP_CLIPBOARD_URL' "$dir/porthop-clip"; then
        echo 'Cannot replace unrelated ~/.local/bin/porthop-clip.' >&2
        exit 1
    fi
fi
tmp="$(mktemp "$dir/.porthop-clip.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
trap 'exit 1' HUP INT TERM
cat > "$tmp"
bash -n "$tmp"
chmod 700 "$tmp"
mv -f "$tmp" "$dir/porthop-clip"
[ -e "$dir/xclip" ] || [ -L "$dir/xclip" ] || ln -s porthop-clip "$dir/xclip"
if [ ! -L "$dir/xclip" ] || [ "$(readlink "$dir/xclip")" != porthop-clip ]; then
    echo 'Existing ~/.local/bin/xclip preserved. Clipboard reads use your existing command.'
elif [ "$(command -v xclip || true)" = "$dir/xclip" ] || { [ -n "$(command -v xclip || true)" ] && [ "$(command -v xclip)" -ef "$dir/xclip" ]; }; then
    echo 'PORTHOP_SHIM_PATH=ready'
else
    echo 'PORTHOP_SHIM_PATH=missing'
fi
