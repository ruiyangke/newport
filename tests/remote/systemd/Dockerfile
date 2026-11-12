# A real systemd server for the Services tab: PID 1 is systemd, so units,
# timers, the journal, lingering user managers, and sudo behave as they do on
# a host. Needs --privileged and the host cgroup namespace.
FROM debian:trixie-slim
ENV container=docker
RUN apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv dbus dbus-user-session openssh-server sudo procps iproute2 \
    && rm -rf /var/lib/apt/lists/* \
    # fixture administers services (journal reader, passwordless systemctl);
    # keeper is an ordinary sudoer who would have to type a password.
    && useradd -m -s /bin/bash -G systemd-journal fixture \
    && useradd -m -s /bin/bash -G sudo keeper \
    && printf 'fixture:unused-container-only-password\nkeeper:unused-container-only-password\n' | chpasswd \
    && printf 'fixture ALL=(root) NOPASSWD: /usr/bin/systemctl\n' > /etc/sudoers.d/fixture \
    && chmod 440 /etc/sudoers.d/fixture \
    && mkdir -p /var/log/journal /var/lib/systemd/linger \
    && touch /var/lib/systemd/linger/fixture \
    && systemctl mask getty@tty1.service console-getty.service systemd-firstboot.service systemd-modules-load.service
COPY sshd_config /etc/ssh/sshd_config
COPY units/ /etc/systemd/system/
COPY --chown=fixture:fixture user/ /home/fixture/.config/systemd/user/
RUN systemctl enable newport-keys.service newport-demo.service newport-broken.service newport-demo.timer \
    && mkdir -p /home/fixture/.config/systemd/user/default.target.wants \
    && ln -s ../newport-user.service /home/fixture/.config/systemd/user/default.target.wants/ \
    && chown -R fixture:fixture /home/fixture/.config
HEALTHCHECK --interval=1s --timeout=3s --start-period=2s --retries=60 CMD systemctl is-active -q newport-keys.service && systemctl is-active -q ssh.service && systemctl is-active -q user@1000.service
STOPSIGNAL SIGRTMIN+3
EXPOSE 22
CMD ["/sbin/init"]
