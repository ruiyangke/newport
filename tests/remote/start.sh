#!/bin/bash
set -euo pipefail
ssh-keygen -A
install -m 600 -o fixture -g fixture /fixture-key/client.pub /home/fixture/.ssh/authorized_keys
chown fixture:fixture /home/fixture/.ssh
chmod 700 /home/fixture/.ssh
newport-test-fixture http 8080 &
exec /usr/sbin/sshd -D -e
