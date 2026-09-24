FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server python3 bash ca-certificates procps iproute2 xclip wl-clipboard \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -s /bin/bash fixture \
    && echo 'fixture:unused-container-only-password' | chpasswd \
    && mkdir -p /run/sshd /home/fixture/.ssh /srv/fixture \
    && printf 'porthop remote fixture\n' > /srv/fixture/index.html
COPY sshd_config /etc/ssh/sshd_config
COPY start.sh /usr/local/bin/start-fixture
EXPOSE 22
CMD ["/bin/bash", "/usr/local/bin/start-fixture"]
