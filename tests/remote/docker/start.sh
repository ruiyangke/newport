#!/bin/sh
set -eu
ssh-keygen -A >/dev/null
for user in fixture outsider; do
    install -d -m 700 -o "$user" -g "$user" "/home/$user/.ssh"
    install -m 600 -o "$user" -g "$user" /fixture-key/client.pub "/home/$user/.ssh/authorized_keys"
done
/usr/sbin/sshd -e
(
    until docker info >/dev/null 2>&1; do sleep 0.2; done
    tar -C /opt/newport-busybox -c . | docker import -c 'CMD ["sh"]' - newport-test/busybox:local >/dev/null
    touch /run/newport-docker-ready
) &
exec dockerd-entrypoint.sh dockerd --host=unix:///var/run/docker.sock --group=docker
