---
title: "Terminals"
description: "Run commands on your server and keep long-running work available through disconnections."
slug: terminals
---

# Terminals

Open **Terminal** for a shell on the selected server. Commands run on that server under your saved SSH account.

## Work with terminal tabs

Use the new-tab control, or **Cmd+T** on macOS, to open another shell. Press **Cmd+1** to **Cmd+8** on macOS, or **Ctrl+Alt+1** to **Ctrl+Alt+8** on Windows, to go to a tab by its position; **Cmd+9** or **Ctrl+Alt+9** goes to the last tab. A tab's menu lets you **Rename**, **Reset name**, **Disconnect**, **Reconnect**, or **Close** it. A custom name can help distinguish a server process, a log viewer, and a working shell.

Switching pages leaves terminal sessions available. Closing or disconnecting a terminal ends its shell connection; remote processes follow the shell's normal job-control rules.

## Copy, paste, and search

| Action | macOS | Windows |
| --- | --- | --- |
| Copy selected text | Cmd+C | Ctrl+Shift+C |
| Paste text | Cmd+V | Ctrl+Shift+V |
| Search terminal output | Cmd+Shift+F | Ctrl+Shift+F |

Use the search controls to move between matches or turn on case matching. Hold **Shift** while selecting text if a terminal application is using the mouse.

**Ctrl+C** retains its shell meaning, usually interrupting the running command. Image attachments use a remote tool's own clipboard action; see [Clipboard and browser sharing](clipboard.md).

## Keep work running through a disconnect

Newport can reconnect after an interruption, but reconnection starts a **new shell**. It cannot restore the old shell's running processes or state.

For persistent work, use tmux or an equivalent tool installed on the server. For example:

```sh
# Start a named session on the server.
tmux new -s work
```

Run your long-running command inside tmux. To leave it running and return to the outer shell, press **Ctrl+B**, release both keys, then press **D**. You can now disconnect Newport.

If `tmux` is not found, install it using your server distribution's package manager or ask the administrator. If a session named `work` already exists, attach to it instead of creating another.

After reconnecting:

```sh
tmux attach -t work
```

Your server and tmux session must still be running. Save files before closing Newport, even when a command runs in tmux.

## Use integrations

Newport terminals receive Needle's environment when it is installed. Enable Clipboard or Browser for the server in **Integrations** to let supported commands use your computer's clipboard or browser.

For graphical applications, complete [Remote apps setup](terminal-gui.md). Once a server is set up, GUI apps you start in its terminals open in windows on your computer. Terminals never wait for this: a terminal always opens as a normal shell, even when graphical apps can't start.

## If a terminal will not connect

Check the server address, authentication, and connection status. Unlock your SSH agent if it needs approval, then retry. Use **Reconnect** from the tab menu when available.

See [Troubleshooting](troubleshooting.md) for SSH and helper errors.

[Back to the user guide](README.md)
