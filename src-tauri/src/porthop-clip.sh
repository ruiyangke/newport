#!/usr/bin/env bash
# Porthop clipboard helper v5
set -euo pipefail
umask 077
root="$HOME/.cache/porthop/clipboard"
snapshot="$root/snapshot.tar"
fail() { printf 'porthop-clip: %s\n' "$*" >&2; exit 1; }
modified() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1"; }
fresh() { [[ -f "$1" ]] && (( $(date +%s) - $(modified "$1") < 120 )); }
native_xclip() {
    local candidate
    [[ "${PORTHOP_CLIPBOARD_NATIVE:-1}" != 0 ]] || return 1
    command -v timeout >/dev/null 2>&1 || return 1
    while IFS= read -r candidate; do
        # Search past our PATH shim, including copies of an older helper.
        [[ "$candidate" -ef "$HOME/.local/bin/porthop-clip" ]] && continue
        grep -q '^# Porthop clipboard helper' "$candidate" 2>/dev/null && continue
        printf '%s\n' "$candidate"; return 0
    done < <(type -aP xclip)
    return 1
}
publish_native() {
    local native display="${DISPLAY:-:0}" kind target=text/plain
    rm -f "$root/native-display"
    if [[ "${PORTHOP_CLIPBOARD_NATIVE:-1}" == 0 ]] || ! command -v timeout >/dev/null 2>&1; then
        echo 'Using the file-backed clipboard.'
        return
    fi
    tmp="$(mktemp "$root/.native.XXXXXX")"
    for kind in text/plain image/png text/html text/uri-list; do
        if tar -xOf "$snapshot" -- "$kind" > "$tmp" 2>/dev/null; then
            target="$kind"; break
        fi
    done
    # wl-copy needs the user's Wayland session environment. Never guess another
    # user's runtime directory. With XDG_RUNTIME_DIR alone, libwayland uses its
    # standard wayland-0 socket. Preserve explicit display/socket selections.
    if [[ -n "${WAYLAND_DISPLAY:-}${WAYLAND_SOCKET:-}${XDG_RUNTIME_DIR:-}" ]] &&
       native="$(type -P wl-copy)"; then
        # Both desktop tools fork clipboard owners: close the lock and SSH output.
        if timeout -k 1 2 "$native" --type "$target" < "$tmp" >/dev/null 2>&1 9>&-; then
            rm -f "$tmp"; tmp=''
            echo 'Wayland clipboard updated.'
            return
        fi
    fi
    if native="$(native_xclip)"; then
        [[ "$target" != text/plain ]] || target=UTF8_STRING
        # Empty/unsupported snapshots replace stale desktop content with empty text.
        if DISPLAY="$display" timeout -k 1 2 "$native" -selection clipboard -in -silent -target "$target" < "$tmp" >/dev/null 2>&1 9>&-; then
            printf '%s\n' "$display" > "$root/native-display"
            printf 'Native X clipboard updated on DISPLAY=%s.\n' "$display"
        fi
    fi
    rm -f "$tmp"; tmp=''
    [[ -f "$root/native-display" ]] || echo 'Desktop clipboard unavailable; using the file-backed clipboard.'
}

# Internal SSH operations. The xclip entry point only exposes reads.
if [[ "${0##*/}" != xclip && "${1:-}" == --* && "${1:-}" != --help ]]; then
    action="$1"; token="${2:-}"
    [[ "$token" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || fail 'Invalid session token.'
    mkdir -p "$root"
    [[ ! -L "$root" && -O "$root" ]] || fail 'Clipboard directory must belong to this account and not be a symlink.'
    chmod 700 "$root"
    tmp=''
    trap '[[ -z "$tmp" ]] || rm -f "$tmp"' EXIT
    trap 'exit 1' HUP INT TERM
    if [[ "$action" == --receive ]]; then
        tmp="$(mktemp "$root/.snapshot.XXXXXX")"
        cat > "$tmp"
        tar -tf "$tmp" >/dev/null || fail 'Incomplete clipboard snapshot.'
    fi
    # Serialize ownership changes, publication, heartbeat and cleanup.
    [[ ! -L "$root/lock" ]] || fail 'Invalid clipboard lock.'
    exec 9>"$root/lock"
    flock -x 9
    owner=''
    [[ ! -f "$root/session" ]] || read -r owner < "$root/session"
    if [[ "$action" == --begin ]]; then
        client="${3:-}"
        [[ -z "$client" || "$client" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || fail 'Invalid client identity.'
        previous_client=''
        [[ ! -f "$root/client" ]] || read -r previous_client < "$root/client"
        if [[ "$owner" != "$token" && ( -z "$client" || "$client" != "$previous_client" ) ]] && fresh "$root/session"; then
            fail 'Clipboard sync is already active for this server account.'
        fi
        rm -f "$snapshot" "$root/native-display"
        printf '%s\n' "$token" > "$root/session"
        printf '%s\n' "$client" > "$root/client"
    elif [[ "$owner" != "$token" ]]; then
        [[ "$action" == --clear ]] || fail 'Clipboard session ended. Enable syncing again.'
    else
        case "$action" in
            --receive) mv -f "$tmp" "$snapshot"; tmp=''; touch "$root/session"; publish_native ;;
            --heartbeat) touch "$root/session"; [[ ! -f "$snapshot" ]] || touch "$snapshot" ;;
            --clear) rm -f "$snapshot" "$root/session" "$root/native-display" "$root/client" ;;
            *) fail 'Unknown internal operation.' ;;
        esac
    fi
    exit 0
fi

target=text/plain
destination=''
if [[ "${0##*/}" == xclip ]]; then
    reading=false
    while (( $# )); do
        case "$1" in
            -o|-out) reading=true ;;
            -i|-in) reading=false ;;
            -selection|-sel|-s)
                (( $# >= 2 )) || fail "Missing value for $1"
                case "$2" in clipboard|primary|secondary|c|p|s) ;; *) fail 'Unknown selection.' ;; esac
                shift ;;
            -target|-t) (( $# >= 2 )) || fail "Missing value for $1"; target="$2"; shift ;;
            -quiet|-silent|-verbose) ;;
            -h|-help|--help) echo 'Porthop read-only xclip: xclip -selection clipboard -o [-t FORMAT]'; exit 0 ;;
            -version|--version) echo 'Porthop xclip shim 3 (read-only)'; exit 0 ;;
            *) fail "Unsupported xclip argument: $1" ;;
        esac
        shift
    done
    "$reading" || fail 'Clipboard writes are not supported. Use xclip -selection clipboard -o to read.'
else
    case "${1:-text}" in
        text|paste) target=text/plain ;;
        html) target=text/html ;;
        urls) target=text/uri-list ;;
        image) target=image/png; destination="${2:-clipboard.png}" ;;
        formats) target=TARGETS ;;
        data) target="${2:?usage: porthop-clip data TYPE [FILE]}"; destination="${3:-}" ;;
        help|-h|--help) echo 'usage: porthop-clip [text|html|urls|image [FILE]|formats|data TYPE [FILE]]'; exit 0 ;;
        *) fail 'Unknown command. Run porthop-clip --help.' ;;
    esac
fi
case "$target" in
    UTF8_STRING|STRING|TEXT|text/plain\;charset=utf-8|text/plain\;\ charset=utf-8|public.utf8-plain-text) target=text/plain ;;
    public.html) target=text/html ;;
    public.png) target=image/png ;;
    public.url|public.file-url) target=text/uri-list ;;
esac
[[ -f "$snapshot" ]] || fail 'No clipboard snapshot. Enable clipboard sync in Porthop.'
# Hold a shared lock so expiry checking and reading refer to the same snapshot.
exec 9<"$root/lock"
flock -s 9
fresh "$snapshot" || fail 'Clipboard snapshot expired. Enable clipboard sync in Porthop.'
# Prefer X only for a display to which this snapshot was successfully published.
# The helper's other entry points always read the mirrored Mac snapshot.
if [[ "${0##*/}" == xclip && "$target" != TARGETS && -f "$root/native-display" ]] &&
   [[ "$(cat "$root/native-display")" == "${DISPLAY:-:0}" ]] && native="$(native_xclip)"; then
    tmp="$(mktemp "$root/.read.XXXXXX")"
    trap 'rm -f "$tmp"' EXIT
    trap 'exit 1' HUP INT TERM
    native_target="$target"
    [[ "$native_target" != text/plain ]] || native_target=UTF8_STRING
    if DISPLAY="${DISPLAY:-:0}" timeout -k 1 2 "$native" -selection clipboard -out -target "$native_target" > "$tmp" 2>/dev/null 9>&-; then
        cat "$tmp"; exit 0
    fi
fi
if [[ -n "$destination" ]]; then
    tar -xOf "$snapshot" -- "$target" > "$destination"
else
    tar -xOf "$snapshot" -- "$target"
fi
