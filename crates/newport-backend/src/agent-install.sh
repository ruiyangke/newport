set -eu
umask 077
dir="$HOME/.local/bin"
mkdir -p "$dir"
tmp="$(mktemp "$dir/.newport-agent.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
trap 'exit 1' HUP INT TERM
cat > "$tmp"
actual="$(sha256sum "$tmp")"
[ "${actual%% *}" = "$1" ] || { echo 'Agent upload checksum mismatch.' >&2; exit 1; }
chmod 700 "$tmp"
[ "$("$tmp" --version)" = "newport-agent/1" ] || exit 1
# Prepare only. Newport verifies the candidate's control handshake before
# requesting activation. Interrupted uploads cannot replace the active build.
"$tmp" deployment prepare "$1"
