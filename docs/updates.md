---
title: "Updates and installation"
description: "Install Newport, keep it up to date, and keep every computer that uses a server on the same version."
slug: updates-and-installation
---

# Updates and installation

Download Newport from [newport.app](https://newport.app/#download). After that, Newport updates itself when you choose to restart. The [release notes](https://newport.app/releases) list what changed.

## Install Newport

| Your computer | Download |
| --- | --- |
| Mac with Apple silicon, macOS 14 or later | [macOS ZIP](https://newport.app/downloads/latest/macos-arm64) |
| Windows, Intel or AMD 64-bit processor | [Windows x64 installer](https://newport.app/downloads/latest/windows-x86_64) |
| Windows, ARM64 processor | [Windows ARM64 installer](https://newport.app/downloads/latest/windows-aarch64) |

On a Mac, open the ZIP and move Newport to Applications. On Windows, run the installer. The Windows installers aren't code-signed yet. There is no Linux version of Newport.

Then follow [Getting started](getting-started.md).

## Update Newport

Newport checks for updates after it starts and every six hours, downloads them, checks their signature, and tells you when one is ready.

1. Open **Settings**, then **Updates**. Choose **Check for updates** to check now.
2. When an update is ready, save your work in project editors and in apps you opened from **Remote apps**, and let file transfers finish.
3. Choose **Restart to update**, then **Restart now**, or **Later** to keep working.

Restarting disconnects your sessions and cancels transfers. Terminal shells keep running on the server and come back after the restart. Your servers and settings are kept.

## Keep every computer on the same version

Newport keeps Needle, its helper on the server, up to date. It checks Needle when it connects and every hour while connected, and installs a new version when one is available.

In **Integrations**, the **Needle** row shows the version this connection uses:

- **Update now** installs an available update.
- **Update** appears when the installed Needle can't work with this Newport, and installs one that can.
- **Details** shows the versions in use.
- **More Needle actions** has **Check again**, **Reinstall** and **Uninstall…**. Uninstalling ends the shells Needle keeps running, so Newport asks first.

Until Newport 1.0, different versions aren't made to work together, so use the same version on every computer that connects to a server. Once one computer updates Needle, an older Newport elsewhere asks you to update it:

- **Integrations** says "Update Newport to manage this agent."
- **Remote apps** says "This server has a newer Newport Remote than this Newport. Update Newport to use it."

Newport doesn't connect to a server just to update Needle, and doesn't reinstall it after you uninstall it.

## If an update does not work

- **"Could not check or download the update. Try again."**: check that your computer can reach the internet, then choose **Check for updates**.
- **"Updates are unavailable in this build."**: this copy of Newport can't update itself. Install Newport from [newport.app](https://newport.app/#download).
- **An older Newport finds no update**: download the current version.
- **Windows warns about the installer**: check that it came from newport.app, and follow your organization's rules for unsigned installers.
- **Newport can't open your saved data after updating**: don't delete anything. Follow [Recover from a startup error](storage.md#recover-from-a-startup-error).

Installing an older version doesn't bring back older data. If a problem continues, [report it](troubleshooting.md#report-a-problem) with the versions you updated from and to.

[Back to the user guide](README.md)
