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

The server does not need the system `xclip` or `wl-clipboard` packages for image
paste in Claude Code. Enabling Clipboard installs Needle's compatible commands,
including when sharing was skipped during initial setup. If an older setup is
missing them, turn Clipboard off and on again. Open a new terminal afterwards,
or run the environment command above and restart Claude Code in that shell.

```sh
xclip -selection clipboard -o
xclip -selection clipboard -o -t image/png > image.png
xclip -o -t TARGETS
```

Like `xdg-open`, the `xclip`, `xsel`, and `wl-paste` aliases serve what Newport provides,
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

### OpenCode v1 and v2

Enable Clipboard for the server and start OpenCode in a new Newport terminal.
Use OpenCode's paste action to attach an image from your computer's clipboard.
Newport's own terminal paste shortcut pastes text; image attachment is handled
by OpenCode reading the shared clipboard.

OpenCode v1 reads PNG images with `wl-paste` or `xclip`, and text through
clipboardy's `xsel`. Needle supplies all three commands, so these reads need no
system clipboard packages. Update Needle and reopen the terminal if `xsel` is
missing from an older installation.

OpenCode v2 uses OpenTUI's native clipboard API through the private X11 or
Wayland display. It needs the corresponding client library on the server:
`libxcb.so.1` for X11 or `libwayland-client.so.0` for Wayland (`libxcb1` and
`libwayland-client0` on Debian/Ubuntu). The command aliases alone do not satisfy
this native API. Keep Needle's display exports when launching OpenCode.

Copying text from either OpenCode version uses the terminal's OSC 52 support.
The Linux SSH compatibility suite runs the released v1.19.4 and v2.1.3 clipboard
adapters with their pinned dependencies, checking text, PNG images, clipboard
changes, empty selections, and both v2 display backends.

## GUI applications from the terminal

Opening a terminal never shows an account dialog or starts a GUI display. Running
an installed GUI application by name, such as `chromium`, checks access and starts
or reuses the display. A licensed user with a ready agent sees the app directly.
Otherwise the command waits while Newport guides sign-in/license activation and,
if needed, Remote apps setup. Completion continues the same process with its
original arguments, working directory and terminal. Cancel or Ctrl+C abandons it;
Newport never inserts or replays shell commands.

See [Terminal GUI flow](terminal-gui.md) for the diagram, failure handling,
command discovery and explicit-path limitations. This needs a current Needle
agent with GUI launch support and the matching Newport Remote agent. Xvfb and
D-Bus are required. Native Wayland-only applications still use Remote apps.

Clipboard and Browser remain separate permissions. Needle's `xclip` and
`wl-paste` commands still read the local clipboard, including screenshot paste
without a host `xclip`. `xdg-open https://example.com` and tools using `BROWSER`
still use the local browser when Browser sharing is enabled; explicitly running
`chromium` starts the remote browser. Update Needle along with Newport Remote so
its shell setup recognizes the streamed display.

The terminal keeps the server user's normal D-Bus environment so Snap, Flatpak,
and `systemctl --user` work. It does not apply application-specific desktop-entry
launch rules to arbitrary shell commands. Packages that force Wayland in their
own environment may need an explicit X11 option, such as Flatpak's
`--env=GDK_BACKEND=x11`.

The display stays available between application launches. Once all its terminal
tabs and windows are gone it is released. Normal shell job control still applies:
use the shell's usual `nohup`/`disown` behavior when a process must survive closing
its originating terminal. A lost GUI connection offers Retry on the next launch;
existing GUI processes cannot move to a replacement display. GUI startup errors
leave the SSH shell intact.
Disabling Clipboard cancels pending transfers and pauses the GUI clipboard bridge
without closing applications. A display first opened with sharing disabled needs
a new GUI display on the next launch to negotiate clipboard support when sharing is enabled.

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
