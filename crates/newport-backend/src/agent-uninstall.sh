# Uninstall Needle: stop it, take out what installing it wrote, and delete
# everything it keeps. Nothing is left and nothing can be restored. Running it
# again finishes an interrupted run. It needs no Needle executable.
set -eu
needle="$HOME/.local/share/newport/needle"
bin="$HOME/.local/bin"
# The entry point is Newport's: whatever runs from it is Needle, such as a
# plain file copied there by hand.
entry="$bin/newport-agent"

# Needle processes of this account, by the executable they run. A fork per
# process takes seconds on a busy server, and one argument per process can
# outgrow the command line: `find` hands `ls` as many as fit, in batches.
needle_processes() {
    find /proc -maxdepth 2 -name exe -path '/proc/[0-9]*/exe' -exec ls -l {} + 2>/dev/null |
        awk -v builds="$needle/builds/" -v entry="$entry" '{
            at = index($0, "/exe -> " builds)
            if (!at && (at = index($0, "/exe -> " entry))) {
                rest = substr($0, at + 8 + length(entry))
                if (rest != "" && rest != " (deleted)") at = 0
            }
            if (!at) next
            pid = substr($0, 1, at - 1)
            sub(/.*\/proc\//, "", pid)
            print pid
        }'
}
# A full agent's session guardian uses the same executable as its workers.
# Leave guardians alive: launcher/worker death tells them to reap detached app
# descendants and unlink their exact X11/Wayland resources. Killing all roles
# together would destroy both cleanup owners and strand those resources.
# Processes still there after five seconds stay in `remaining`; removal goes
# on without them, and the end reports them.
stop_needle() {
    attempts=0
    while :; do
        remaining=$(needle_processes)
        [ -n "$remaining" ] && [ "$attempts" -lt 20 ] || return 0
        for pid in $remaining; do
            # Recheck the executable after enumeration, before reading the role.
            executable=$(readlink "/proc/$pid/exe" 2>/dev/null) || continue
            case "$executable" in
            "$needle"/builds/*) ;;
            "$entry"|"$entry (deleted)") ;;
            *) continue ;;
            esac
            role=$(tr '\000' '\n' 2>/dev/null < "/proc/$pid/cmdline" | sed -n '2p')
            if [ "$role" = '--session-guard' ]; then
                # A stopped guardian must run to observe its launcher's death.
                kill -CONT "$pid" 2>/dev/null || true
            else
                kill -KILL "$pid" 2>/dev/null || true
            fi
        done
        attempts=$((attempts + 1))
        sleep 0.25
    done
}

# Newport, here or on another computer, may start Needle again at any time,
# through its entry point. Without it nothing can.
for name in xdg-open newport-browser; do
    link="$bin/$name"
    [ -L "$link" ] || continue
    target=$(readlink "$link")
    case "$target" in
    newport-agent) rm -f "$link" ;;
    "$needle"/builds/*/newport-agent)
        hash=${target#"$needle"/builds/}
        hash=${hash%/newport-agent}
        [ "${#hash}" -eq 64 ] || continue
        case "$hash" in *[!0-9a-f]*) ;; *) rm -f "$link" ;; esac
        ;;
    esac
done
# A link is removed, never what it points to.
if [ -L "$entry" ] || [ -f "$entry" ]; then
    rm -f "$entry"
fi
stop_needle

# Newport's block in a startup file, between its markers. The file is
# replaced atomically with its permissions; links are never followed.
strip_block() {
    file=$1
    [ -f "$file" ] && [ ! -L "$file" ] || return 0
    grep -q '^# >>> Newport >>>$' "$file" || return 0
    staged=$(mktemp "$file.newport.XXXXXX")
    awk '/^# >>> Newport >>>$/ { skip = 1; next }
        /^# <<< Newport <<<$/ { skip = 0; next }
        !skip' "$file" >"$staged"
    chmod "$(stat -c %a "$file")" "$staged"
    mv -f "$staged" "$file"
}
for file in .bashrc .bash_profile .bash_login .profile .zshrc; do
    strip_block "$HOME/$file"
done
if [ -n "${ZDOTDIR:-}" ]; then strip_block "$ZDOTDIR/.zshrc"; fi
fish="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/newport.fish"
strip_block "$fish"
# The fish file is Newport's own; without its block it holds nothing else.
if [ -f "$fish" ] && [ ! -L "$fish" ] && ! grep -q '[^[:space:]]' "$fish"; then
    rm -f "$fish"
fi

rm -rf "$needle" "$HOME/.cache/newport" "$HOME/.local/state/newport"
# Anything that started before its build was deleted.
stop_needle
rmdir "$HOME/.local/share/newport" 2>/dev/null || true
for runtime in "${XDG_RUNTIME_DIR:-}" "/run/user/$(id -u)"; do
    if [ -n "$runtime" ]; then rm -rf "$runtime/newport-ipc"; fi
done
if [ -n "$remaining" ]; then
    printf 'Needle is removed, but its session cleanup is still running (process %s). Uninstall again once it ends.\n' \
        "$(printf '%s' "$remaining" | tr '\n' ' ' | sed 's/ $//')" >&2
    exit 1
fi
printf 'uninstalled\n'
