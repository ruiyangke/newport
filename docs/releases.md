---
title: "Updates and installation"
description: "Choose a Newport download, install updates, and resolve installation problems."
slug: updates-and-installation
---

# Updates and installation

Download Newport from [newport.app](https://newport.app/#download). Read the [release notes](https://newport.app/releases) to see what changed.

## Choose a download

| Your computer | Download |
| --- | --- |
| macOS 14 or later, Apple silicon | [macOS ZIP](https://newport.app/downloads/latest/macos-arm64) |
| Windows, Intel or AMD 64-bit processor | [Windows x64 installer](https://newport.app/downloads/latest/windows-x86_64) |
| Windows, ARM64 processor | [Windows ARM64 installer](https://newport.app/downloads/latest/windows-aarch64) |

On macOS, extract the ZIP and move Newport to Applications. On Windows, run the installer for your processor.

Windows support is experimental. A minimum supported Windows version has not yet been established; the processor choices above are not a promise of compatibility with every Windows release. The installers are not Authenticode-signed. There is no Linux desktop build. The servers you connect to run Linux.

For your first connection, follow [Getting started](getting-started.md).

## Update from Newport

Newport downloads available updates automatically. You choose when to restart.

1. Open **Settings → Updates** to see the installed version and update status.
2. Choose **Check for updates** if you want to check now.
3. When an update is ready, save your editor and remote-app work and finish any file transfers.
4. Choose **Restart to update**, then **Restart now**. Choose **Later** to keep working.

Restarting disconnects active sessions and cancels transfers. A terminal reconnect starts a fresh shell; use a remote session manager such as tmux when work needs to survive disconnection.

Newport verifies update signatures before installing. A successful update keeps your saved servers and preferences. See [Storage and recovery](storage.md) for how credentials and upgrade backups are protected.

## Update Needle on a server

Needle is Newport's remote helper. After you approve its installation, Newport checks it when connecting and periodically while connected. Open **Integrations** to see its status or update it manually.

Several computers, or several Newport versions, can use the same server. Each runs its own Needle version for its sessions. Newport changes the server's default version automatically only when it is an older release, or when no other connected Newport is using it.

If the server has a newer signed helper than your desktop app supports, update Newport first. Checks do not reconnect a disconnected server or reinstall a helper you uninstalled.

## If an update does not work

- **The check or download fails:** confirm that your computer can reach the internet, then retry from Settings.
- **An older Newport version finds no update:** download the current installer from the website. Older releases may use a different update source.
- **Updates are unavailable in this build:** install a distributed version from the website to use automatic updates.
- **Windows shows an installation warning:** check that the file came from Newport's website and matches your processor. Follow your organization's policy for unsigned installers.
- **Newport cannot open saved data after updating:** preserve the data directory and its credential-store entries. See [Recovery](storage.md#recover-from-a-startup-or-vault-error) before changing anything.

Installing an older application version does not automatically restore an older copy of your data. For persistent problems, include the versions you updated from and to in a [problem report](troubleshooting.md#report-a-problem).

[Back to the user guide](README.md)
