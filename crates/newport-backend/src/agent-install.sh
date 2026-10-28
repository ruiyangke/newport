set -eu
umask 077
dir="$HOME/.local/bin"
mkdir -p "$dir"
tmp="$(mktemp "$dir/.newport-agent.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
trap 'exit 1' HUP INT TERM
case "${2-}" in
    gzip)
        command -v gzip >/dev/null 2>&1 || { echo 'gzip is required for this agent upload.' >&2; exit 1; }
        gzip -dc > "$tmp" || { echo 'Cannot decompress agent upload.' >&2; exit 1; }
        ;;
    raw) cat > "$tmp" ;;
    *) echo 'Unsupported agent upload encoding.' >&2; exit 1 ;;
esac
actual="$(sha256sum "$tmp")"
[ "${actual%% *}" = "$1" ] || { echo 'Agent upload checksum mismatch.' >&2; exit 1; }
chmod 700 "$tmp"
[ "$("$tmp" --version)" = "newport-agent/1" ] || exit 1
# Prepare only. Newport verifies the candidate's control handshake before
# requesting activation. Interrupted uploads cannot replace the active build.
"$tmp" deployment prepare "$1"
