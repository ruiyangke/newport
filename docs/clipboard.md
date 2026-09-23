# Clipboard sync and the Porthop agent

Enable **Integration → Clipboard** to share your Mac's clipboard with a Linux server. Porthop installs one `porthop-agent` binary for x86_64 or ARM64 and starts it over SSH. The agent handles clipboard storage, headless X11 and Wayland clipboards, and requests to open links on your Mac.

Sharing is one-way and remembered across app launches. Only enable it for trusted servers: copied passwords and other sensitive content are included, and enabling **Browser** lets processes running as the server account request browser tabs. Clipboard and Browser have independent switches; the shared agent stays connected while either is on.

## Read the clipboard

On the server:

```sh
xclip -selection clipboard -o
xclip -selection clipboard -o -t image/png > image.png
xclip -o -t TARGETS
```

The agent installs `xclip`, `wl-paste`, `xdg-open`, and `porthop-browser` aliases in `~/.local/bin`. Existing unrelated commands are preserved. If needed, put that directory first on PATH:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

You can bypass a preserved command with `~/.local/bin/porthop-agent clipboard -o`. The aliases support clipboard reads, not clipboard writes or primary selection.

## Headless image paste

Applications such as Codex read display protocols directly. While sharing is active, the agent serves both X11 and Wayland without a graphical desktop.

Add this to your headless shell's startup file:

```sh
eval "$("$HOME/.local/bin/porthop-agent" env)"
```

This sets PATH and the environment for currently enabled integrations: the browser command for Browser, and a fixed Wayland socket plus the active X11 display and authority file for Clipboard. Run it once in the current shell too, then start a new Codex process or tmux pane. An existing process keeps its previous environment.

The Wayland socket is always `~/.cache/porthop/clipboard/wayland.sock`. If a reconnect changes the X11 display number, rerun the environment command before starting another X11-only application. These are clipboard-only displays; do not use their environment for graphical applications.

For an isolated command, the same binary can create a temporary display:

```sh
porthop-agent display --backend x11 -- codex
porthop-agent display --backend wayland -- codex
```

Porthop manages the agent's lifetime; no systemd service is needed. If you previously installed the standalone Wayland service, stop it once so the agent can own the fixed socket:

```sh
systemctl --user disable --now porthop-clipboard-wayland.service
```

## Open links on your Mac

Enable **Integration → Browser**, then run the shell configuration shown in the app.

```sh
xdg-open https://example.com
```

On headless sessions, the alias sends the URL over SSH to your Mac's default browser. On graphical desktops, it delegates to the next native `xdg-open` on PATH. Use `porthop-agent open URL` to explicitly choose your Mac.

The environment command above also configures tools that use `BROWSER`. To configure browser opening alone:

```sh
export PATH="$HOME/.local/bin:$PATH"
export BROWSER="$HOME/.local/bin/porthop-browser"
```

For AWS SSO on the server:

```sh
aws sso login --profile NAME --use-device-code
```

Device authorization works without a callback listener. Leave browser opening enabled. See [AWS's SSO guide](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-sso.html).

Browser login URLs with an explicit HTTP loopback `redirect_uri` automatically get a temporary SSH forward from the Mac callback address and port to the server. Porthop binds both IPv4 and IPv6 for `localhost`. The forward expires after 5 minutes or when Integration disconnects. If forwarding cannot be set up, the browser still opens and the helper prints a warning. Use the CLI's device-code or paste-code fallback if needed.

Porthop does not guess hidden callback ports. HTTPS callbacks and listeners inside a separate container network require another setup or the CLI's remote-login fallback.

Only HTTP and HTTPS URLs without embedded credentials are accepted. Requests are sent immediately, with no stored queue or browser polling. A successful opener command means the Mac accepted the browser open request; it does not mean authorization completed. If sharing is disconnected or a request is rate-limited, reconnect or retry.

## Desktop clipboards

When available in the SSH session, the agent publishes through native `wl-copy` or `xclip`. Wayland needs the user's session environment. X11 uses `DISPLAY`, falling back to `:0` when unset, and preserves `XAUTHORITY`. Native tools are optional; headless reads always use the stored snapshot. Each native call has a two-second deadline.

The snapshot holds up to 32 MiB, prioritizing text, PNG, HTML, and URLs. Extra representations that do not fit are skipped. If no format fits, the remote snapshot is cleared. Native desktop publishing selects one format; the agent's clipboard services expose all stored formats.

## Reconnection and cleanup

One agent owns each server account's clipboard. A single persistent SSH channel carries clipboard updates to Linux and browser requests back to your Mac. Private Unix sockets serve local agent clients. Browser authentication may create temporary Mac loopback listeners; inbound Mac SSH access is not needed.

Temporary SSH failures retry after 2, 4, 8, 16, then at most 30 seconds. Permission, protocol, and ownership errors stop with an error message. Temporary Mac clipboard-read timeouts retry without dropping the agent connection. Turning an integration off cancels in-flight work immediately. Disabling both integrations stops retries and the agent. Disabling one restarts the agent with only the remaining permission.

Snapshots and sockets live in the account-only `~/.cache/porthop/clipboard` directory. The agent removes its snapshot and sockets on normal shutdown or SSH EOF, and stops after 45 seconds without incoming data from Porthop. Upload progress keeps it alive; clipboard uploads have a two-minute deadline, and stalled output is bounded to five seconds. A force-killed agent may leave files; clipboard readers reject snapshots older than two minutes. Native desktop clipboard content is not cleared on disconnect.

Installed binaries and aliases remain for reuse. Porthop compares the bundled binary's checksum before uploading an update and verifies each upload before activation. Changing a profile's host, port, or username turns both integrations off until enabled for the new destination.

To repair the installation, choose **Integration → Reinstall agent**. Porthop replaces its binary, repairs its aliases, and reconnects the integrations you enabled. Your switches remain unchanged.
