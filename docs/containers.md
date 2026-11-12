---
title: "Containers and Compose"
description: "Inspect containers on your server, manage Compose projects, open container ports, and free disk space."
slug: containers-and-compose
---

# Containers and Compose

Open **Containers** to see the Docker or Podman containers on the selected server. Read their logs, open a shell in them, run Compose projects, open their web ports on your computer, and free disk space.

Your SSH account must be allowed to use Docker. Compose projects need Docker Compose on the server.

## Work with a container

Find a container in the list and select it. **Running** shows running containers and any that need attention; **All** adds stopped ones.

- The buttons under its name start, stop, restart or resume it. **Stop** asks first.
- **Shell** opens a terminal tab with a shell inside the running container.
- **…** has **Pause**, **Remove…** for a stopped container, and **Recreate** for a Compose service. Removing keeps the image and volumes. Recreating loses files kept only inside the container.
- **Logs** follows and searches its output. **Inspect** shows its command, environment, mounts and networks; environment values stay hidden until you choose **Show values**.

## Open a container's web port

A running container's published ports appear under **Ports**. Click one to forward it to your computer and open it in your browser. The forward is saved in [Connections](connections.md).

Only published TCP ports can be opened, and not ones published only on one network address, such as `192.168.1.10`. Ports that can't be opened are greyed out; hold the pointer over one to see why. For another port, add a port mapping to the container's Docker or Compose configuration.

## Run a Compose project

Select a project in the list.

- **Up** creates missing containers and recreates what changed in the Compose files.
- **Restart all** and **Stop all** act on the existing containers, and ask first.
- **…** pulls or builds images, and has **Down…**.
- **Services** lists each service, with its own **Up**, **Recreate** and **Scale…**. **Activity** shows each command's output.

**Down…** removes the project's containers and networks; **Up** creates them again. If you also delete the project's volumes, their data is gone for good, so Newport asks you to type the project name.

## Free disk space

Choose **Disk usage** at the top right to see what images, containers, volumes and build cache use. Each cleanup button says what it deletes and asks first. **Remove unused** deletes only unused anonymous volumes; named volumes stay.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| **No Docker on this server** or **Docker isn't answering** | Install or start Docker on the server, then choose **Try again**. |
| **This account can't use Docker** | Choose **Add me to the docker group…**, check the command typed into the terminal tab, and press **Return**. This gives the account root-level access, from its next login. |
| **Podman's socket is off** | Choose **Turn on Podman's socket…** and press **Return** in the terminal tab. |
| **Shell** is unavailable | Start the container. |
| **Up** and **Down…** are missing | Install Docker Compose on the server. |
| A forwarded port doesn't respond | Check that the container is running, and read its **Logs**. |

Logs and environment values can contain secrets. Remove them before you share them.

[Back to the user guide](README.md)
