# Uninstall Needle: stop it, take out what installing it wrote, and delete
# everything it keeps. Nothing is left and nothing can be restored. Running it
# again finishes an interrupted run. It needs no Needle executable.
set -eu
needle="$HOME/.local/share/newport/needle"
bin="$HOME/.local/bin"
# An earlier Needle installed as a plain file, by hand or before builds were
# managed: it names Needle's protocol. Read, never run, to tell.
legacy=""
if [ -f "$bin/newport-agent" ] && [ ! -L "$bin/newport-agent" ] &&
    grep -q 'newport-agent/[0-9]' "$bin/newport-agent"; then
    legacy="$bin/newport-agent"
fi

# Needle processes of this account, by the executable they run. A fork per
# process takes seconds on a busy server, and one argument per process can
# outgrow the command line: `find` hands `ls` as many as fit, in batches.
needle_processes() {
    find /proc -maxdepth 2 -name exe -path '/proc/[0-9]*/exe' -exec ls -l {} + 2>/dev/null |
        awk -v builds="$needle/builds/" -v legacy="$legacy" '{
            at = index($0, "/exe -> " builds)
            if (!at && legacy != "" && (at = index($0, "/exe -> " legacy))) {
                rest = substr($0, at + 8 + length(legacy))
                if (rest != "" && rest != " (deleted)") at = 0
            }
            if (!at) next
            pid = substr($0, 1, at - 1)
            sub(/.*\/proc\//, "", pid)
            print pid
        }'
}
# Everything Needle keeps is deleted below, so it has nothing to save: KILL.
stop_needle() {
    for pid in $(needle_processes); do kill -KILL "$pid" 2>/dev/null || true; done
}

# Newport, here or on another computer, may start Needle again at any time,
# through its entry point. Without it nothing can.
if [ -L "$bin/newport-agent" ]; then
    case "$(readlink "$bin/newport-agent")" in
    "$needle"/builds/*) rm -f "$bin/newport-agent" ;;
    esac
elif [ -n "$legacy" ]; then
    rm -f "$legacy"
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

# The command links that point at the entry point.
for name in xclip xsel wl-paste xdg-open newport-browser; do
    link="$bin/$name"
    [ -L "$link" ] || continue
    case "$(readlink "$link")" in
    newport-agent) rm -f "$link" ;;
    esac
done

rm -rf "$needle" "$HOME/.cache/newport" "$HOME/.local/state/newport"
# Anything that started before its build was deleted.
stop_needle
rmdir "$HOME/.local/share/newport" 2>/dev/null || true
for runtime in "${XDG_RUNTIME_DIR:-}" "/run/user/$(id -u)"; do
    if [ -n "$runtime" ]; then rm -rf "$runtime/newport-ipc"; fi
done
printf 'uninstalled\n'
