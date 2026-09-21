# Clipboard sync

Clipboard sync sends your Mac's clipboard to a selected server over SSH. It is one-way and opt-in for each server. Enable it in **Connections → Clipboard sync**; the setting is remembered across app launches.

Only share with servers you trust. Clipboard contents can include passwords, tokens and personal information. The server account and privileged users can read the remote copy.

## Read the clipboard

On the server:

```sh
xclip -selection clipboard -o
```

To save a PNG image or list available formats:

```sh
xclip -selection clipboard -o -t image/png > image.png
xclip -o -t TARGETS
```

Porthop installs a small receiver and a read-only `xclip` shim in `~/.local/bin`. The server needs Bash, tar, flock and standard file utilities. Existing executables are preserved; conflicts are reported instead of overwritten.

If the app reports a PATH issue, add this to your server shell's startup file, such as `~/.bashrc` or `~/.zshrc`, then open a new shell:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

You can also run `~/.local/bin/xclip -selection clipboard -o` directly. The app checks PATH in the SSH execution environment, which may differ from your interactive shell.

## Desktop and headless servers

| Environment | Behavior |
| --- | --- |
| Wayland | Uses `wl-copy` when installed and a Wayland session is accessible. Read with `wl-paste`. |
| X11 | Uses native `xclip`, preserving `DISPLAY` and `XAUTHORITY`. If DISPLAY is unset, it tries `:0`. |
| Headless or inaccessible desktop | The shim reads the stored snapshot without a display. |

Wayland needs the appropriate `WAYLAND_DISPLAY` or `WAYLAND_SOCKET` environment and, for relative sockets, `XDG_RUNTIME_DIR`. With only `XDG_RUNTIME_DIR`, the default socket is `wayland-0`. Porthop does not guess runtime directories or install desktop clipboard tools.

Desktop publishing falls back from Wayland to X11, then to the stored snapshot. Each native call has a two-second deadline plus a one-second kill grace period. Set `PORTHOP_CLIPBOARD_NATIVE=0` in the remote execution environment to force snapshot-only behavior.

The snapshot holds all captured formats, up to 32 MiB total. Desktop publishing selects one format per update: text, PNG, HTML, then URLs. The shim can read additional snapshot formats, but it does not implement every xclip option or accept clipboard writes. Its primary and secondary selections also refer to the shared clipboard.

## Reconnection and cleanup

Temporary transport failures reconnect automatically after 2, 4, 8, 16, then at most 30 seconds between attempts. Permission and ownership errors stop with an error message. **Disable sync** stops retries and saves the off state.

One client owns a server account's snapshot at a time. A persistent client identity lets the same profile reconnect; a new connection token prevents late writes or cleanup from an older connection. Another client cannot take over a live session. Older sessions without a matching client identity may take two minutes to expire.

Snapshots are stored at `~/.cache/porthop/clipboard/snapshot.tar`, with owner-only directory and file permissions. A heartbeat keeps unchanged content available. The shim refuses stale content two minutes after the last update or heartbeat.

Disabling sync or quitting attempts to remove the snapshot. Expiration does not guarantee file deletion after a lost connection. Installed helpers remain for reuse, and native desktop clipboard content is not cleared on disconnect. Changing the profile's host, port or username turns sharing off until you enable it for the new destination.
