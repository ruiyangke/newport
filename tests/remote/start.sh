#!/bin/bash
set -euo pipefail
ssh-keygen -A
for user in fixture zsh-fixture; do
    install -m 600 -o "$user" -g "$user" /fixture-key/client.pub "/home/$user/.ssh/authorized_keys"
    chown "$user:$user" "/home/$user/.ssh"
    chmod 700 "/home/$user/.ssh"
done
newport-test-fixture http 8080 &
exec /usr/sbin/sshd -D -e
