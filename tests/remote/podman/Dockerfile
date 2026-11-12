FROM busybox:1.37 AS busybox

FROM fedora:44
RUN dnf install -y --setopt=install_weak_deps=False podman openssh-server \
        iproute curl-minimal shadow-utils coreutils \
    && dnf clean all \
    && groupadd docker \
    && useradd --create-home --shell /bin/bash --groups docker fixture \
    && printf 'fixture:unused-container-only-password\n' | chpasswd \
    && mkdir -p /fixture-key
COPY --from=busybox / /opt/newport-busybox/
COPY storage.conf /etc/containers/storage.conf
COPY containers.conf /etc/containers/containers.conf
COPY sshd_config /etc/ssh/sshd_config
COPY start.sh /usr/local/bin/start-fixture
COPY docker /usr/local/bin/docker
RUN chmod 0755 /usr/local/bin/docker
HEALTHCHECK --interval=1s --timeout=3s --start-period=2s --retries=90 CMD test -e /run/newport-podman-ready
EXPOSE 22
CMD ["/bin/sh", "/usr/local/bin/start-fixture"]
