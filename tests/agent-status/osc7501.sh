#!/bin/sh
# Plays a coding agent that reports its status through OSC 7501, the Program
# Status Protocol, to check by eye how Newport's tabs show it. Run it in a
# Newport terminal on any server; it needs only sh, stty, dd and base64.
#
#   osc7501.sh probe                      Asks whether the terminal speaks OSC 7501.
#   osc7501.sh working [PERCENT] [TEXT]   Spinner.
#   osc7501.sh blocked KIND [TEXT]        Yellow dot. KIND: permission, question or auth.
#   osc7501.sh done [TEXT]                Green dot on a tab you are not looking at.
#   osc7501.sh error [TEXT]               Red dot.
#   osc7501.sh idle                       No mark.
#   osc7501.sh task ID STATE [TEXT]       A background task's entry, beside the program's.
#   osc7501.sh clear [ID]                 Clears one task, or everything.
#   osc7501.sh tour [SECONDS]             Every mark in turn, then clears.
#   osc7501.sh killed [SECONDS]           Reports working, then dies without clearing.
#   osc7501.sh hostile                    Reports the tab must clean up or ignore.
#
# To drive a tab from another shell on the server, send the reports to that
# tab's terminal, as `osc7501.sh done > /dev/pts/3`; `tty` in the tab names it.
# `probe` and `killed` must run in the tab itself: `probe` reads the answer
# from it, and `killed` needs a process group of its own in its foreground.
set -eu

app=newport-test

b64() {
    printf '%s' "$1" | base64 | tr -d '\n'
}

# One report, ended with ST.
# shellcheck disable=SC1003 # The backslash is ST's second byte.
emit() {
    printf '\033]7501;%s\033\\' "$1"
}

# A report with an optional message, which travels as base64.
report() {
    fields="$1"
    text="${2:-}"
    if [ -n "$text" ]; then
        fields="$fields:msg=$(b64 "$text")"
    fi
    emit "$fields"
}

say() {
    printf '%s\n' "$*"
}

pause() {
    sleep "$1"
}

# Asks as Claude Code does: the question, then a DA1 request every terminal
# answers. A terminal that speaks the protocol answers the question first.
probe() {
    if [ ! -t 0 ]; then
        say "probe needs a terminal on standard input" >&2
        exit 2
    fi
    saved=$(stty -g)
    trap 'stty "$saved"' EXIT INT TERM
    stty raw -echo min 0 time 10
    printf '\033]7501;?\033\\\033[c'
    sleep 1
    reply=$(dd bs=512 count=1 2>/dev/null | tr '\033' 'E')
    stty "$saved"
    trap - EXIT INT TERM
    before_da1=${reply%%"E[?"*}
    case "$before_da1" in
    *"E]7501;?"*) say "supported: the terminal answered OSC 7501 before DA1" ;;
    *)
        say "not supported: no OSC 7501 answer before DA1"
        exit 1
        ;;
    esac
}

tour() {
    step=${1:-5}
    say "Switch to another tab and back as it runs to see the green dot."
    say "working: a spinner"
    report "state=working:app=$app"
    pause "$step"
    say "working at 40%: a spinner; hold the pointer over the tab for the progress"
    report "state=working:app=$app:progress=40" "Reading the test suite"
    pause "$step"
    say "blocked on permission: a yellow dot, 'Waiting for your permission'"
    report "state=blocked:app=$app:kind=permission" "Run: rm -rf build"
    pause "$step"
    say "blocked on a question: a yellow dot, 'Waiting for your answer'"
    report "state=blocked:app=$app:kind=question" "Which branch should I use?"
    pause "$step"
    say "blocked on sign-in: a yellow dot, 'Waiting for you to sign in'"
    report "state=blocked:app=$app:kind=auth"
    pause "$step"
    say "working with a task waiting: the yellow dot wins over the spinner"
    report "state=working:app=$app"
    report "state=blocked:app=$app:id=review:kind=question" "Review the plan?"
    pause "$step"
    say "the task cleared: back to the spinner"
    emit "state=clear:id=review"
    pause "$step"
    say "done: a green dot while you look elsewhere; it goes when you look"
    report "state=done:app=$app" "All 42 tests pass"
    pause "$step"
    say "error: a red dot"
    report "state=error:app=$app" "The build failed"
    pause "$step"
    say "idle: no mark"
    report "state=idle:app=$app"
    pause "$step"
    emit "state=clear"
    say "cleared"
}

# Dies the way a killed agent does, leaving its status behind. Newport clears
# it once this process group is gone, within a second.
killed() {
    report "state=working:app=$app" "About to be killed"
    say "working; killing this process in ${1:-5} seconds without clearing"
    pause "${1:-5}"
    kill -9 $$
}

hostile() {
    step=2
    say "control and direction characters: the hint shows plain text"
    report "state=blocked:app=$app:kind=question" \
        "$(printf 'Line one\nLine two\033[31m red?\342\200\256 reversed\001')"
    pause "$step"
    say "a message over 2048 characters: the hint is cut short"
    long=$(printf '%3000s' '' | tr ' ' 'x')
    report "state=working:app=$app" "$long"
    pause "$step"
    say "a report over 8 KiB: ignored, the spinner stays"
    huge=$(printf '%9000s' '' | tr ' ' 'y')
    emit "state=error:app=$app:msg=$huge"
    pause "$step"
    say "an unknown state, a bad id and a bad app: ignored, the spinner stays"
    emit "state=exploded:app=$app"
    emit "state=error:id=bad/id:app=$app"
    emit "state=error:app=bad app"
    pause "$step"
    say "70 tasks: kept to 64; the yellow dot from one of them shows"
    i=0
    while [ "$i" -lt 70 ]; do
        emit "state=blocked:app=$app:id=task$i:kind=question"
        i=$((i + 1))
    done
    pause "$step"
    say "ended with BEL instead of ST: a red dot"
    printf '\033]7501;state=error:app=%s\007' "$app"
    pause "$step"
    emit "state=clear"
    say "cleared"
}

command=${1:-}
[ $# -gt 0 ] && shift
case "$command" in
probe) probe ;;
working)
    fields="state=working:app=$app"
    if [ -n "${1:-}" ]; then
        fields="$fields:progress=$1"
    fi
    report "$fields" "${2:-}"
    ;;
blocked) report "state=blocked:app=$app:kind=${1:?kind: permission, question or auth}" "${2:-}" ;;
"done" | error) report "state=$command:app=$app" "${1:-}" ;;
idle) report "state=idle:app=$app" ;;
task) report "state=${2:?state}:app=$app:id=${1:?id}" "${3:-}" ;;
clear) emit "state=clear${1:+:id=$1}" ;;
tour) tour "${1:-5}" ;;
killed) killed "${1:-5}" ;;
hostile) hostile ;;
*)
    sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
