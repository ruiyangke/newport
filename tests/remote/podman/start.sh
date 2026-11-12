#!/bin/sh
set -eu
tr -d '-' < /proc/sys/kernel/random/uuid > /etc/machine-id
ssh-keygen -A >/dev/null
install -d -m 700 -o fixture -g fixture /home/fixture/.ssh
install -m 600 -o fixture -g fixture /fixture-key/client.pub /home/fixture/.ssh/authorized_keys
tar -C /opt/newport-busybox -c . | podman import -c 'CMD ["sh"]' - newport-test/busybox:local >/dev/null
podman system service --time 0 unix:///var/run/docker.sock &
until test -S /var/run/docker.sock; do sleep 0.1; done
chgrp docker /var/run/docker.sock
chmod 0660 /var/run/docker.sock
touch /run/newport-podman-ready
exec /usr/sbin/sshd -D -e
