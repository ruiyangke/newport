ARG RUST_VERSION=1.98.0
FROM rust:${RUST_VERSION}-bookworm AS probe
WORKDIR /probe
COPY arboard-probe/Cargo.toml arboard-probe/Cargo.lock ./
COPY arboard-probe/src ./src
RUN cargo build --release --locked
WORKDIR /fixture
COPY fixture/Cargo.toml fixture/Cargo.lock ./
COPY fixture/src ./src
RUN cargo build --release --locked

FROM oven/bun:1.4.4 AS opencode-probe
WORKDIR /opencode-probe
COPY opencode-probe/package.json opencode-probe/bun.lock ./
RUN bun install --frozen-lockfile --ignore-scripts
COPY opencode-probe ./

# The CLI backend requires Git with --attr-source (Git 2.43+).
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server git bash zsh fish ca-certificates procps iproute2 xclip wl-clipboard direnv \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -s /bin/bash fixture \
    && useradd -m -s /usr/bin/zsh -p '*' zsh-fixture \
    && echo 'fixture:unused-container-only-password' | chpasswd \
    && mkdir -p /run/sshd /home/fixture/.ssh /home/zsh-fixture/.ssh /srv/fixture \
    && printf 'newport remote fixture\n' > /srv/fixture/index.html
COPY --from=probe /probe/target/release/newport-arboard-probe /usr/local/bin/arboard-probe
COPY --from=opencode-probe /usr/local/bin/bun /usr/local/bin/bun
COPY --from=opencode-probe /opencode-probe /opt/opencode-probe
COPY sshd_config /etc/ssh/sshd_config
COPY --from=probe /fixture/target/release/newport-test-fixture /usr/local/bin/newport-test-fixture
COPY start.sh /usr/local/bin/start-fixture
HEALTHCHECK --interval=1s --timeout=2s --start-period=1s --retries=30 CMD test -s /home/fixture/.ssh/authorized_keys && ss -ltn | grep -q ":22 " && ss -ltn | grep -q ":8080 "
EXPOSE 22
CMD ["/bin/bash", "/usr/local/bin/start-fixture"]
