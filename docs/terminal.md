---
title: "Terminals"
description: "Open uninterruptible shells on your server that keep running after you quit Newport, and use them from more than one computer."
slug: terminals
---

# Terminals

Open **Terminal** for a shell on the selected server. Commands run on the server under your SSH account.

## Uninterruptible terminals

Your shells keep running on the server, like tmux, with nothing to start or attach. Quit Newport, lose the connection, close your laptop or choose **Disconnect**: the shell and its programs carry on. Every computer connected to the server shows the same shells.

When you come back, the tab shows output printed while you were away, with up to 5,000 lines of scrollback.

Opening a server's terminals shows the shells running there and selects the tab this computer had selected, or else the newest shell. Newport opens a new shell only when the server has none.

A shell ends only when:

- You choose **Close**. Its programs end too. Newport asks first if a command is still running. The last tab can't be closed.
- The shell exits, for example after `exit`. Its tab closes, or shows **Shell exited** and the exit code if this happened while you were away.
- The server restarts. The tab shows **Shell ended on the server**; choose **Reconnect** for a new shell.

Keeping shells running needs Needle on the server; see [Set up the helper](getting-started.md#set-up-the-helper). Without it, a terminal is an ordinary shell that ends when you disconnect.

If Needle can't start the shell for a new tab, the tab opens an ordinary shell instead, with a **This shell won't keep running** notice giving the reason. That shell ends when the connection drops or you quit Newport.

Terminals in the project editor are not kept. They end when you close them or the editor, or quit Newport.

## Work with tabs

- Choose **+** (**New terminal**), or press **Cmd+T** on macOS, to open another shell.
- The tab strip scrolls to show the selected tab and its close button when you open a tab, its title changes, or you resize the window. You can also scroll the strip to browse other tabs.
- Press **Cmd+1** to **Cmd+8** on macOS, or **Ctrl+Alt+1** to **Ctrl+Alt+8** on Windows, to go to a tab. **Cmd+9** or **Ctrl+Alt+9** goes to the last one.
- Right-click a tab for **Rename**, **Reset name**, **Reconnect**, **Disconnect** and **Close**, or double-click it to rename it.
- Copy, paste and search with **Cmd+C**, **Cmd+V** and **Cmd+Shift+F** on macOS, or **Ctrl+Shift+C**, **Ctrl+Shift+V** and **Ctrl+Shift+F** on Windows. **Ctrl+C** interrupts the running command.
- In the find bar, **Previous match** and **Next match** move between results, as do **Shift+Return** and **Return**. **Match case** matches capital letters exactly.
- Hold **Shift** while you select text if a program is using the mouse.

## More terminal controls

- Choose **Enter immersive mode**, to the right of the tabs, to hide the sidebar and title bar so the terminal fills the window. Choose **Exit immersive mode** in the same place to leave it. Terminals in the project editor don't have it.
- Cmd-click a link on macOS, or Ctrl-click it on Windows, to open it in your computer's browser. This works without **Browser links**. A link to the server itself, such as `http://localhost:3000`, opens through a forward that Newport adds in [Connections](connections.md).

## Use a server's shells from several computers

A server's shells belong to the server. Every computer with Newport connected to it shows them as tabs, in the order they started.

- A shell opened on another computer appears as a new tab at the end, without changing your selected tab.
- Several computers can use one shell at once. They see the same screen, and any of them can type.
- Hold the pointer over a tab to see which other computers have it open.
- Links, clipboard requests and the shell's window size follow the computer where you last typed, pasted or clicked in it.
- Renaming a tab renames it everywhere. **Close** ends the shell on every computer; if another computer has it open, Newport asks first.

## Use your clipboard, browser and Linux apps

To let programs in a terminal use your clipboard or browser, turn on **Clipboard access** or **Browser links** for the server in **Integrations**. See [Clipboard and browser sharing](clipboard.md).

With [Remote apps](remote-apps.md) set up, Linux apps you run in a terminal open in windows on your computer, and typing in a terminal brings the server's apps here. A terminal never waits for apps.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| A tab won't connect | See [Cannot connect over SSH](troubleshooting.md#cannot-connect-over-ssh). |
| A tab waits after your SSH agent declined the login | Press **Return** in the tab to try again. It also retries when your computer wakes, unlocks or changes network. |
| A shell ended when you quit or disconnected | The server may end your processes when you log out; ask its administrator. |
| A tab says "This shell won't keep running" | Newport couldn't keep the shell on the server, for the reason shown, so the tab has an ordinary shell that ends when you disconnect. If the reason names a directory, remove it or run `chmod 700` on it in that shell, then open a new tab. |
| A tab says "This terminal uses an incompatible protocol." | An older Needle build holds the shell. Choose **Close** on the tab to end it, then open a new tab. |
| A command says "Open this terminal in Newport to continue." | No computer has the shell open. Open its tab and run the command again. |
| A link or paste fails and names another computer | Type in the tab on this computer, then try again. |
| **New terminal** is unavailable | A server can have 32 shells, counting every computer's. Close one. |

[Back to the user guide](README.md)
