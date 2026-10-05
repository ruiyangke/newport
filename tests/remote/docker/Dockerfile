# A real Docker Engine for the Containers tab. dockerd runs inside this
# privileged container (Docker-in-Docker), so tests may stop, remove and prune
# freely without touching the host's engine, containers or images. `fixture`
# is in the docker group; `outsider` is not. A busybox root filesystem is
# imported as an image at start, so tests never pull from a registry.
FROM busybox:1.37 AS busybox

FROM docker:29-dind
ENV DOCKER_TLS_CERTDIR=""
RUN apk add --no-cache openssh-server bash coreutils grep \
    && mkdir -p /fixture-key \
    && adduser -D -s /bin/bash fixture \
    && adduser -D -s /bin/bash outsider \
    && addgroup fixture docker \
    && printf 'fixture:unused-container-only-password\noutsider:unused-container-only-password\n' | chpasswd
COPY --from=busybox / /opt/newport-busybox/
COPY sshd_config /etc/ssh/sshd_config
COPY start.sh /usr/local/bin/start-fixture
HEALTHCHECK --interval=1s --timeout=3s --start-period=2s --retries=90 CMD test -e /run/newport-docker-ready && nc -z 127.0.0.1 22
EXPOSE 22
ENTRYPOINT []
CMD ["/bin/sh", "/usr/local/bin/start-fixture"]
