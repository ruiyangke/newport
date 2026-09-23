# Porthop agent

One Linux binary for Porthop's remote clipboard and browser integration.

Porthop deploys and runs the agent automatically when Clipboard or Browser is enabled in Integration. It provides read-only X11 and Wayland clipboard services, `xclip` / `wl-paste` aliases, and an `xdg-open` alias that opens headless web requests on your Mac. See the [clipboard guide](../../docs/clipboard.md).

## Build

With Rust and Python 3 installed, run from the repository root:

```sh
npm run build:agent
```

This builds static x86_64 and ARM64 Linux binaries using a pinned Zig toolchain, then places them in `src-tauri/agents` for embedding in the Mac app. Tauri development and release builds run this step automatically. Run it before invoking Cargo directly for the Mac app.

For a local build and tests:

```sh
cargo test --locked --manifest-path tools/agent/Cargo.toml
```

To install from source on Linux:

```sh
cargo install --locked --path tools/agent --root "$HOME/.local"
porthop-agent install
```

## Commands

- `serve CLIENT_ID [--clipboard] [--browser]`: framed clipboard and browser transport on stdin/stdout, managed by Porthop over SSH.
- `install`: install aliases, preserving unrelated commands.
- `env`: print the headless shell environment.
- `clipboard -o [-t FORMAT]`: read a snapshot format.
- `open URL`: open an HTTP(S) URL on your Mac, optionally forwarding a login callback.
- `display --backend x11|wayland -- COMMAND`: run a command with an isolated clipboard display.

The persistent transport is versioned and size-bounded. It uses atomic snapshots, private local sockets, exclusive ownership, and a heartbeat timeout. Browser logins with an explicit HTTP loopback `redirect_uri` get a temporary listener on the same Mac address and port, forwarded over the existing SSH connection. `localhost` binds both IPv4 and IPv6. Listeners expire after 5 minutes or when Integration disconnects. Forwarding failures produce a warning while allowing the browser to open.

Device-code URLs need no forwarding. Other callback schemes and container-private listeners are not supported automatically. Porthop preserves the login URL and leaves token exchange to the remote CLI.
