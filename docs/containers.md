---
title: "Containers and Compose"
description: "Inspect Docker containers, manage Compose services, and review storage usage on your server."
slug: containers-and-compose
---

# Containers and Compose

Open **Containers** to manage Docker workloads on the selected server. Needle and Docker must be available, and your SSH account must be able to reach the Docker daemon. Compose operations also need Docker Compose on that server.

The Docker **daemon** is the background service that runs containers. **Compose** manages a related group of containers from a configuration file.

## Inspect a container

Select a container to see its state, resource usage, environment, mounts, and networks. Open its logs to follow output and search for messages. Choose **Shell** to open a shell inside a running container when the image provides one.

Use the available lifecycle actions to start, stop, or restart a container. Read confirmation dialogs before removing a container or changing a running workload.

Environment values and logs can contain secrets. Remove sensitive values before including them in a support report.

## Reach a container's service

Use a container's published-port controls to forward a service to your computer. A published port is the port Docker exposes on the server; it can differ from the port inside the container.

If the application is listening only inside the container and has no published server port, configure its Docker or Compose port mapping first. See [Connections](connections.md) for manual forwarding and local addresses.

## Work with a Compose project

Newport groups Compose services by project. Open a project to inspect its configuration, services, and activity.

- Use **Up** to create a missing service container.
- Use **Recreate** to replace a service container using its current configuration.
- Use **Pull & recreate** when you also want to fetch its image before recreating it.
- Inspect project activity and logs when an operation fails or a service does not become healthy.

Recreating a container replaces that container; it does not preserve files stored only inside that container (its writable layer). Keep application data in the appropriate persistent storage and review the operation before proceeding.

## Inspect disk usage

Choose **Disk usage** to see space used by containers, images, volumes, and build cache. Cleanup actions show what they will remove before you confirm.

A volume is storage managed separately from a container so its data can survive replacement. An unused volume can still contain data you care about, and a stopped container can still contain files. Back up needed data before removing either. Removing images or build cache may make later starts or builds slower while content is downloaded or rebuilt.

## Troubleshooting

| Problem | What to try |
| --- | --- |
| Docker is unavailable | Check that Docker is installed and its daemon is running on the server |
| Docker access is denied | Confirm the SSH account has the intended Docker access; ask the server administrator if needed |
| You changed group membership but access still fails | Reconnect so the server creates a session with the new permissions |
| A container has no usable shell | The image may not include a shell; inspect its logs or use an image-specific diagnostic method |
| A Compose action is unavailable | Check that Compose is installed and the project's configuration files remain accessible |
| A forwarded app does not respond | Check the published server port, the container's state, and the application logs |

If Newport offers **Add me to the docker group…**, review the terminal command before running it. Docker access grants substantial control over the server; follow the server administrator's access policy.

[Back to the user guide](README.md)
