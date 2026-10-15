listeners=$(ss -tlnp 2>/dev/null || netstat -tlnp 2>/dev/null) || { echo 'Port discovery requires ss or netstat on the server.' >&2; exit 1; }
# Reuse existing non-interactive sudo permission; never prompt or alter sudoers.
if elevated=$(sudo -n ss -tlnp 2>/dev/null || sudo -n netstat -tlnp 2>/dev/null); then
    listeners="$listeners
$elevated"
fi
printf '%s\n' "$listeners"
printf '__NEWPORT_DOCKER_PORTS__\n'
docker_rows=$(docker ps --format '{"name":{{json .Names}},"ports":{{json .Ports}},"image":{{json .Image}},"project":{{json (.Label "com.docker.compose.project")}},"service":{{json (.Label "com.docker.compose.service")}},"workdir":{{json (.Label "com.docker.compose.project.working_dir")}}}' 2>/dev/null) || true
[ -z "$docker_rows" ] || printf '%s\n' "$docker_rows"
# Encode fields so paths, whitespace and NUL-delimited argv cannot corrupt records.
hex() { od -An -v -tx1 | tr -d ' \n'; }
start_time() { sed 's/.*) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }
# Directories that hold projects but are never one themselves.
boundary() {
    case $1 in
        /home/*/*|/root/*) return 1 ;;
        ''|/|/home|/home/*|/root|/tmp|/var|/srv|/opt|/mnt|/media) return 0 ;;
    esac
    return 1
}
# Projectile's root order: .projectile, the nearest VCS marker, the topmost
# project file, then the top of the nearest Makefile chain. Prints root|marker.
project_root() {
    case $1 in
        /usr|/usr/*|/bin|/bin/*|/sbin|/sbin/*|/etc|/etc/*|/proc|/proc/*|/sys|/sys/*|/dev|/dev/*|/run|/run/*|/nix/store|/nix/store/*|/var/lib|/var/lib/*|/var/log|/var/log/*) return ;;
    esac
    pr_dir=$1 pr_depth=0 pr_local='' pr_vcs='' pr_vcs_marker='' pr_top='' pr_top_marker='' pr_make='' pr_make_done=''
    while [ "$pr_depth" -lt 32 ] && ! boundary "$pr_dir"; do
        [ -z "$pr_local" ] && [ -e "$pr_dir/.projectile" ] && pr_local=$pr_dir
        if [ -z "$pr_vcs" ]; then
            for pr_marker in .git .hg .jj .sl .fslckout _FOSSIL_ .bzr _darcs .svn; do
                if [ -e "$pr_dir/$pr_marker" ]; then pr_vcs=$pr_dir pr_vcs_marker=$pr_marker; break; fi
            done
        fi
        for pr_marker in package.json Cargo.toml go.mod pyproject.toml setup.py requirements.txt Gemfile composer.json pom.xml build.gradle build.gradle.kts mix.exs deno.json deno.jsonc flake.nix stack.yaml dune-project build.sbt project.clj deps.edn pubspec.yaml Package.swift CMakeLists.txt compose.yaml compose.yml docker-compose.yml docker-compose.yaml; do
            if [ -e "$pr_dir/$pr_marker" ]; then pr_top=$pr_dir pr_top_marker=$pr_marker; break; fi
        done
        if [ -z "$pr_make_done" ]; then
            if [ -e "$pr_dir/Makefile" ]; then pr_make=$pr_dir; elif [ -n "$pr_make" ]; then pr_make_done=1; fi
        fi
        pr_dir=${pr_dir%/*} pr_depth=$((pr_depth + 1))
    done
    if [ -n "$pr_local" ]; then printf '%s|.projectile' "$pr_local"
    elif [ -n "$pr_vcs" ]; then printf '%s|%s' "$pr_vcs" "$pr_vcs_marker"
    elif [ -n "$pr_top" ]; then printf '%s|%s' "$pr_top" "$pr_top_marker"
    elif [ -n "$pr_make" ]; then printf '%s|Makefile' "$pr_make"
    fi
}
# Sets gw_here (the checkout), gw_top (the repository it belongs to), gw_branch
# and gw_main for the checkout at $1, whose root holds .git. A linked
# worktree's .git file names its gitdir, whose commondir leads to the
# repository every worktree shares. Reads files; needs no git binary.
git_worktree() {
    gw_here='' gw_top='' gw_branch='' gw_main=''
    if [ -d "$1/.git" ]; then
        gw_dir=$1/.git
    elif [ -f "$1/.git" ]; then
        gw_dir=$(sed -n 's/^gitdir: //p' "$1/.git" 2>/dev/null | head -n 1)
        [ -n "$gw_dir" ] || return
        case $gw_dir in /*) ;; *) gw_dir=$1/$gw_dir ;; esac
        gw_dir=$(cd "$gw_dir" 2>/dev/null && pwd -P) || return
    else
        return
    fi
    gw_here=$(cd "$1" 2>/dev/null && pwd -P) || gw_here=$1
    gw_top=$gw_here gw_main=1
    # A submodule's gitdir has no commondir; it is its own main checkout.
    if [ -f "$gw_dir/commondir" ]; then
        gw_common=$(head -n 1 "$gw_dir/commondir" 2>/dev/null)
        case $gw_common in /*) ;; *) gw_common=$gw_dir/$gw_common ;; esac
        if gw_common=$(cd "$gw_common" 2>/dev/null && pwd -P); then
            gw_main=0
            case $gw_common in */.git) gw_top=${gw_common%/.git} ;; *) gw_top=$gw_common ;; esac
        fi
    fi
    gw_head=$(head -n 1 "$gw_dir/HEAD" 2>/dev/null)
    case $gw_head in
        'ref: refs/heads/'*) gw_branch=${gw_head#ref: refs/heads/} ;;
        'ref: '*) gw_branch=${gw_head#ref: } ;;
        *) gw_branch=$(printf '%s' "$gw_head" | cut -c 1-7) ;;
    esac
}
# Prints hex(root)|hex(marker)|hex(worktree)|hex(branch)|hex(main flag) for $1.
project_record() {
    pi_found=$(project_root "$1")
    pi_root=${pi_found%|*} pi_marker=${pi_found##*|} gw_here='' gw_top='' gw_branch='' gw_main=''
    pi_worktree=''
    if [ "$pi_marker" = .git ]; then
        git_worktree "$pi_root"
        if [ -n "$gw_top" ]; then pi_worktree=$gw_here pi_root=$gw_top; fi
    fi
    printf '%s|%s|%s|%s|%s' "$(printf '%s' "$pi_root" | hex)" "$(printf '%s' "$pi_marker" | hex)" \
        "$(printf '%s' "$pi_worktree" | hex)" "$(printf '%s' "$gw_branch" | hex)" "$(printf '%s' "$gw_main" | hex)"
}
printf '__NEWPORT_COMPOSE_ROOTS__\n'
# A Compose directory may sit inside a repository or one of its worktrees.
# Escaped JSON paths are skipped rather than unescaped; they keep the default.
printf '%s\n' "$docker_rows" | sed -n 's/.*"workdir":"\([^"\\]*\)"}$/\1/p' | sort -u | head -n 64 | while IFS= read -r workdir; do
    [ -n "$workdir" ] || continue
    printf '%s|%s\n' "$(printf '%s' "$workdir" | hex)" "$(project_record "$workdir")"
done
printf '__NEWPORT_PROCESS_DETAILS__\n'
roots=''
printf '%s\n' "$listeners" | sed -n -e 's/.*pid=\([0-9][0-9]*\).*/\1/p' -e 's/.*LISTEN[[:space:]]*\([0-9][0-9]*\)\/.*/\1/p' | sort -nu | head -n 256 | while read -r pid; do
    before=$(start_time "$pid")
    [ -n "$before" ] || continue
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null | head -c 4096 | hex)
    cwd_path=$(readlink "/proc/$pid/cwd" 2>/dev/null | head -c 4096)
    cwd=$(printf '%s' "$cwd_path" | hex)
    args=$(head -c 4096 "/proc/$pid/cmdline" 2>/dev/null | hex)
    user=$(ps -p "$pid" -o user= 2>/dev/null | head -c 256 | hex)
    [ "$before" = "$(start_time "$pid")" ] || continue
    # Processes often share a directory; resolve each one once.
    root=$(printf '%s\n' "$roots" | grep -m 1 "^$cwd|" | cut -d '|' -f 2-)
    # Linux marks a directory removed under a running process " (deleted)".
    # Walking up from it would credit the process to whatever contained it.
    case $cwd_path in *' (deleted)') [ -d "$cwd_path" ] || root='||||' ;; esac
    if [ -z "$root" ] && [ -n "$cwd" ]; then
        root=$(project_record "$cwd_path")
        roots="$roots
$cwd|$root"
    fi
    [ -n "$root" ] || root='||||'
    printf '%s|%s|%s|%s|%s|%s\n' "$pid" "$exe" "$cwd" "$args" "$user" "$root"
done
