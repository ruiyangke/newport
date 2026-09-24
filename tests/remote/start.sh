#!/bin/bash
set -euo pipefail
ssh-keygen -A
install -m 600 -o fixture -g fixture /fixture-key/client.pub /home/fixture/.ssh/authorized_keys
chown fixture:fixture /home/fixture/.ssh
chmod 700 /home/fixture/.ssh
python3 -m http.server 8080 --bind 127.0.0.1 --directory /srv/fixture &
exec /usr/sbin/sshd -D -e
