# Clipboard and browser sharing

In **Integrations**, set up Needle and enable **Clipboard** or **Browser** for
the server. The switches are independent and remembered across app launches.
Clipboard sharing is one-way, from your computer to server applications. Enable
it only for trusted servers: copied passwords and other sensitive content are
included. Browser sharing lets processes under the server account request tabs.

## Terminal setup

Needle installs owned aliases in `~/.local/bin` and a managed shell configuration
block. Existing unrelated commands and files are preserved. Use the terminal
setup shown on the Integrations page, then open a new shell. For an existing
Bash/Zsh shell:

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
preserves an existing graphical desktop environment. Already-running programs
retain their old environment; start a new process or tmux pane after refreshing.

## Read the clipboard

```sh
xclip -selection clipboard -o
xclip -selection clipboard -o -t image/png > image.png
xclip -o -t TARGETS
```

The `xclip` and `wl-paste` aliases support reads, not writes or primary selection.
If a real command was preserved, use `~/.local/bin/newport-agent clipboard -o`
directly. Content is fetched on demand and cached in memory until the clipboard
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
environment command. Reopen applications that retained an old display environment.

Use the Integrations page to inspect connection or setup errors and review
recovery actions. Reinstallation is an explicit operation; do not remove helper
journals or backups manually. See [Needle deployment](../tools/agent/README.md#deployment-and-recovery)
and [storage and recovery](storage.md).

On macOS, desktop diagnostics are in
`~/Library/Logs/app.newport/newport.log`. Remote diagnostics accompany the selected
integration's private runtime directory; there is no single fixed socket path
for all sessions. Logs record formats, sizes, and errors, not clipboard contents.
A successful transfer does not establish that the receiving app pasted it.
