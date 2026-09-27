ARG RUST_VERSION=1.98.0
FROM rust:${RUST_VERSION}-bookworm AS probe
WORKDIR /probe
COPY arboard-probe/Cargo.toml arboard-probe/Cargo.lock ./
COPY arboard-probe/src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server python3 bash zsh fish ca-certificates procps iproute2 xclip wl-clipboard \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -s /bin/bash fixture \
    && echo 'fixture:unused-container-only-password' | chpasswd \
    && mkdir -p /run/sshd /home/fixture/.ssh /srv/fixture \
    && printf 'porthop remote fixture\n' > /srv/fixture/index.html
COPY --from=probe /probe/target/release/porthop-arboard-probe /usr/local/bin/arboard-probe
COPY sshd_config /etc/ssh/sshd_config
COPY shell_setup.py /srv/fixture/shell_setup.py
COPY start.sh /usr/local/bin/start-fixture
HEALTHCHECK --interval=1s --timeout=2s --start-period=1s --retries=30 CMD test -s /home/fixture/.ssh/authorized_keys && ss -ltn | grep -q ":22 " && ss -ltn | grep -q ":8080 "
EXPOSE 22
CMD ["/bin/bash", "/usr/local/bin/start-fixture"]
