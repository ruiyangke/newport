---
title: "Settings"
description: "Change Newport's theme and startup, arrange the sidebar, and find keyboard shortcuts."
slug: settings
---

# Settings

**Settings** holds the choices that apply to all of Newport, not to one server. Choose **Settings** at the bottom of the sidebar, above your account.

You can also open it from Newport's icon in the menu bar on macOS or the system tray on Windows: choose **Settings…**. On macOS, it is also in the **Newport** menu.

## Appearance

**Theme** sets the colors: **Follow system**, **Light** or **Dark**.

## Terminal

On macOS, **Use Option as Meta key** is on at first. Option+F, Option+B and Option+D then move and delete by word in the shell. Turn it off to type special characters with Option.

## Startup

**Launch at login** starts Newport in the background when you sign in to your computer.

## Updates

**Updates** shows your version and checks for new ones. See [Updates and installation](updates.md).

## Cache

**Clear cache** deletes the recorded **Metrics history** for all servers. See [Server monitoring](monitoring.md#clear-the-history).

## About Newport

On macOS, choose **Newport** > **About Newport**. Windows has no About window; your version is under **Updates** in **Settings**.

## Arrange the sidebar

- Choose the sidebar button at the left of the window header to switch between page names and icons only.
- Drag the sidebar's edge to make it wider or narrower. Double-click the edge to reset it.
- On macOS, choose **Workspace** > **Show/hide sidebar** to hide it. Choose the sidebar button to show it again.

## Keyboard shortcuts

For terminal shortcuts, such as new tabs and copy and paste, see [Terminals](terminal.md#work-with-tabs).

In the project editor:

- **Cmd+S** (**Ctrl+S** on Windows): save the file.
- **Cmd+P** (**Ctrl+P**): find a file by name.
- **Cmd+W** (**Ctrl+W**): close the tab.
- **Cmd+B** (**Ctrl+B**): show or hide the file tree.
- **Cmd+Shift+E** (**Ctrl+Shift+E**): open and focus the file tree.
- **Cmd+1** to **Cmd+9** (**Ctrl+Alt+1** to **Ctrl+Alt+9**): go to a tab. 9 goes to the last one.
- **Ctrl+Tab** and **Ctrl+Shift+Tab**: next and previous tab.
- **Ctrl+\`**: show or hide the project terminal.
- **F2**: **Rename symbol…**
- **Shift+F12**: **Find references**.
- **Shift+Option+F** (**Shift+Alt+F**): **Format document**.

Apart from the tab numbers, these keys go to the shell while the project terminal has focus. See [Language tools](language-servers.md) for renaming, references and formatting.

## Close and quit

Closing the window keeps Newport and its connections running. To bring the window back, choose **Open Newport** from Newport's icon in the menu bar or system tray. To quit, choose **Quit Newport** there, or in the **Newport** menu on macOS. See [Quit Newport](getting-started.md#quit-newport).

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| **Launch at login** says "Available in the installed app." | This copy of Newport, such as a development build, can't start at login. Use Newport from [newport.app](https://newport.app/#download). |
| Option types special characters instead of moving by word | Turn on **Use Option as Meta key**. |
| The sidebar is gone | Choose the sidebar button at the left of the window header. |

[Back to the user guide](README.md)
