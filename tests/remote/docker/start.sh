#!/bin/sh
set -eu
ssh-keygen -A >/dev/null
for user in fixture outsider; do
    install -d -m 700 -o "$user" -g "$user" "/home/$user/.ssh"
    install -m 600 -o "$user" -g "$user" /fixture-key/client.pub "/home/$user/.ssh/authorized_keys"
done
/usr/sbin/sshd -e
if [ "${NEWPORT_TEST_SECOND_ENGINE:-0}" = 1 ]; then
    dockerd --host=unix:///var/run/docker-replacement.sock --group=docker \
        --data-root=/var/lib/docker-replacement --exec-root=/var/run/docker-replacement \
        --pidfile=/var/run/docker-replacement.pid --bridge=none \
        --iptables=false --ip6tables=false --ip-forward=false --ip-masq=false \
        --userland-proxy=false &
fi
(
    until docker info >/dev/null 2>&1; do sleep 0.2; done
    if [ "${NEWPORT_TEST_SECOND_ENGINE:-0}" = 1 ]; then
        until docker --host unix:///var/run/docker-replacement.sock info >/dev/null 2>&1; do sleep 0.2; done
    fi
    tar -C /opt/newport-busybox -c . | docker import -c 'CMD ["sh"]' - newport-test/busybox:local >/dev/null
    touch /run/newport-docker-ready
) &
exec dockerd-entrypoint.sh dockerd --host=unix:///var/run/docker.sock --group=docker "$@"
