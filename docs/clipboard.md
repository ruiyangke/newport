# Clipboard sync and the Porthop agent

Enable **Integration → Clipboard** to share your Mac's clipboard with a Linux server. Porthop installs one `porthop-agent` binary for x86_64 or ARM64 and starts it over SSH. The agent handles on-demand clipboard reads, headless X11 and Wayland clipboards, and requests to open links on your Mac.

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

## On-demand clipboard reads

Copying sends only a revision and available formats over SSH. Clipboard content stays on your computer until a server app requests it. Porthop then reads that format and transfers it in chunks. Chunks of at least 1 KiB are losslessly compressed with zlib when that makes them smaller; incompressible data stays raw. Decompression is bounded to 64 KiB per chunk and the 32 MiB clipboard limit. The agent caches requested formats in memory, up to 32 MiB total, and clears the cache on the next copy or disconnect. Simultaneous reads of the same format share one fetch.

macOS checks the clipboard change counter every 200 ms without reading its contents. Windows uses clipboard-change notifications, with counter polling as a fallback. Cached reads can briefly return the previous copy before its change notification arrives; they do not currently validate the desktop revision on every paste. The first paste may take longer for large images; Porthop must remain connected. Failed requests time out after 15 seconds and can be retried without restarting the agent.

Use the installed `xclip`/`wl-paste` aliases or configure the agent's X11/Wayland displays with the environment command above. Porthop does not push copies into a separate native desktop clipboard. Applications using that desktop's display need to use the agent's display to read shared content.

## Reconnection and cleanup

One agent owns each server account's clipboard. A single persistent SSH channel carries clipboard metadata, content requests and responses, and browser requests. Private Unix sockets serve local agent clients. Browser authentication may create temporary Mac loopback listeners; inbound Mac SSH access is not needed.

Temporary SSH failures retry after 2, 4, 8, 16, then at most 30 seconds. Permission, protocol, and ownership errors stop with an error message. Temporary Mac clipboard-read timeouts retry without dropping the agent connection. Turning an integration off cancels in-flight work immediately. Disabling both integrations stops retries and the agent. Disabling one restarts the agent with only the remaining permission.

Sockets and diagnostic logs live in the account-only `~/.cache/porthop/clipboard` directory. On-demand clipboard contents are not written to disk. The agent removes its sockets on normal shutdown or SSH EOF, and stops after 45 seconds without incoming data from Porthop. Heartbeats continue during clipboard transfers. A force-killed agent may leave socket files; the next session handles stale sockets. Existing snapshot files from older versions are removed on startup.

Installed binaries and aliases remain for reuse. Porthop compares the bundled binary's checksum before uploading an update and verifies each upload before activation. Changing a profile's host, port, or username turns both integrations off until enabled for the new destination.

To repair the installation, choose **Integration → Reinstall agent**. Porthop replaces its binary, repairs its aliases, and reconnects the integrations you enabled. Your switches remain unchanged.

## Troubleshooting intermittent paste failures

After a failed paste, note the time and check **Integration** for a connection error. On the server, inspect recent clipboard events:

```sh
tail -n 100 ~/.cache/porthop/clipboard/diagnostics.log
```

On macOS, desktop events are in `~/Library/Logs/ke.ry.porthop/porthop.log`. Search for `Clipboard` to follow format announcements and requested captures. Remote events show clipboard revisions, X11 and Wayland requests, stale offers, and transfer outcomes. A successful transfer does not confirm that the receiving app pasted the image.

Logs contain timestamps, sizes, format metadata, and error categories—not clipboard contents. The remote log resets when it reaches 256 KiB, survives reconnects, and can be deleted. Logging failures do not stop sync.
