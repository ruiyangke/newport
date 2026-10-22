# Clipboard and browser sharing

In **Integrations**, set up Needle and enable **Clipboard** or **Browser** for
the server. The switches are independent and remembered across app launches.
Clipboard sharing is one-way, from your computer to server applications. Enable
it only for trusted servers: copied passwords and other sensitive content are
included. Browser sharing lets processes under the server account request tabs.

## Terminal setup

Newport's own terminals start the login shell with Needle's exports, so
sharing works there with no setup. Needle also installs owned aliases in
`~/.local/bin` and a managed shell configuration block, for terminals Newport
didn't open (plain `ssh`, tmux started elsewhere) and for PATH where a system
profile resets it in login shells (Debian, Ubuntu, NixOS). Existing unrelated
commands and files are preserved. For an existing Bash/Zsh shell:

```sh
if [ -x "$HOME/.local/bin/newport-agent" ]; then
  if _newport_env=$("$HOME/.local/bin/newport-agent" env 2>/dev/null); then
    eval "$_newport_env"
  fi
  unset _newport_env
fi
```

Fish uses `newport-agent env --shell fish`. The command supplies PATH, BROWSER,
and the selected session's clipboard display environment as appropriate. It
preserves an existing graphical desktop environment. Every value stays valid
for as long as a shell runs, so tmux panes that live for weeks keep working:
PATH and BROWSER name commands that find the live session when they run, the
session ID is the same each time this computer connects, and display paths go
through one name per computer that Needle moves to each new session. Without a
usable session, it still puts Needle's commands on PATH.

## Copy from the server

In Newport's terminal, programs can copy text to your computer's clipboard the
way terminals allow (OSC 52): copying in tmux (mouse selection or copy mode, with
its default `set-clipboard external`), in Vim or Neovim with an OSC 52 clipboard,
and similar. Only writing plain text to the standard clipboard is accepted, up
to 8 MiB; nothing in the terminal can read or clear your clipboard. To select
the screen's text directly while a program uses the mouse, hold Shift.

## Read the clipboard

```sh
xclip -selection clipboard -o
xclip -selection clipboard -o -t image/png > image.png
xclip -o -t TARGETS
```

Like `xdg-open`, the `xclip` and `wl-paste` aliases serve what Newport provides,
reading your computer's clipboard, and hand everything else to the server's
own command when it has one: writes, the primary selection, other options, and
a graphical desktop of the server's own. If a real command was preserved ahead
of them on PATH, use `~/.local/bin/newport-agent clipboard -o` directly. Content is fetched on demand and cached in memory until the clipboard
changes or sharing disconnects. Newport must remain connected.

For applications that read X11 or Wayland directly, the environment command
selects a private clipboard display. Do not hard-code its socket or display
number. These are clipboard-only displays, not graphical desktops. A command can
also use an isolated display:

```sh
newport-agent display --backend x11 -- COMMAND
newport-agent display --backend wayland -- COMMAND
```

## Open browser links

```sh
xdg-open https://example.com
```

In a headless session, the installed alias sends the URL to your computer's
browser. On a graphical desktop it delegates to the next native `xdg-open` on
PATH. `newport-agent open https://example.com` explicitly uses Newport. Only
HTTP(S) URLs without embedded credentials are accepted.

The environment command sets BROWSER for tools that honor it. Browser login URLs
with an explicit HTTP loopback `redirect_uri` get a temporary SSH callback
forward. Device-code logins need no callback. Hidden callback ports, HTTPS
callbacks, and container-private listeners require separate setup. A successful
open request does not mean authentication completed.

## Reconnection and troubleshooting

Newport retries transient connection failures. Permission changes take effect
without restarting the agent; disabling sharing cancels pending reads/opens.
Other agent features may continue running while both sharing switches are off.

If an old terminal cannot find its session:

```sh
printf 'NEWPORT_SESSION=%s\n' "${NEWPORT_SESSION-}"
newport-agent sessions
```

Known previous session identities reconcile to the same client's new connection.
An ambiguous destination or disabled integration is not silently bypassed. Select
an intended active session from the list when necessary, then rerun the
environment command.

A shell from an older Needle may hold an earlier session's display paths;
running `eval "$(newport-agent env)"` in it moves it to the live session.
Programs it already started need restarting once.

Use the Integrations page to inspect connection or setup errors, and to
install, update or uninstall Needle. See
[Needle install and uninstall](../tools/agent/README.md#install-and-uninstall).

On macOS, desktop diagnostics are in
`~/Library/Logs/app.newport/newport.log`. Remote diagnostics accompany the selected
integration's private runtime directory; there is no single fixed socket path
for all sessions. Logs record formats, sizes, and errors, not clipboard contents.
A successful transfer does not establish that the receiving app pasted it.
