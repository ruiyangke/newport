# syntax=docker/dockerfile:1
# Keep the Rust version aligned with rust-toolchain.toml. The digest pins all platforms.
FROM rust:1.98.0-alpine3.23@sha256:4743b6231029d726d7a0f81d730a7c9f4eff23225a4499c01e275efb5e260235 AS toolchain
ARG RUST_VERSION=1.98.0
RUN test "$(rustc --version | cut -d ' ' -f 2)" = "$RUST_VERSION"
RUN apk add --no-cache build-base file
WORKDIR /build

FROM toolchain AS checked-runtime
COPY tests/agent-runtime/string-boundary.c /tmp/string-boundary.c
RUN cc -O2 -static /tmp/string-boundary.c -o /tmp/runtime-check && /tmp/runtime-check

FROM scratch AS runtime-check
COPY --from=checked-runtime /tmp/runtime-check /runtime-check

FROM checked-runtime AS source
# Laid out as in the repository: the agent uses the desktop's terminal engine
# and display contract by path.
WORKDIR /build/crates/newport-agent
COPY crates/newport-agent/Cargo.toml crates/newport-agent/Cargo.lock crates/newport-agent/build.rs ./
COPY resources/app.json /build/resources/app.json
COPY crates/newport-agent/src ./src
COPY crates/newport-terminal/Cargo.toml /build/crates/newport-terminal/Cargo.toml
COPY crates/newport-terminal/src /build/crates/newport-terminal/src
COPY crates/newport-domain/Cargo.toml /build/crates/newport-domain/Cargo.toml
COPY crates/newport-domain/src /build/crates/newport-domain/src
COPY crates/newport-protocol /build/crates/newport-protocol

FROM source AS build
ARG TARGETARCH
# A value, not a file's modification time, is what makes the cached build
# scripts run again when the version changes.
ARG NEWPORT_AGENT_VERSION
ENV NEWPORT_AGENT_VERSION=$NEWPORT_AGENT_VERSION
# Where in the cache, which every checkout shares, this build compiles. Cargo
# goes by modification times, so a clean build empties its directory first and
# after; the locked cache keeps any other build out meanwhile.
ARG NEWPORT_AGENT_BUILD=shared
RUN --mount=type=cache,id=newport-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=newport-agent-${TARGETARCH},target=/build/crates/newport-agent/target,sharing=locked \
    export CARGO_TARGET_DIR="target/$NEWPORT_AGENT_BUILD" && \
    if [ "$NEWPORT_AGENT_BUILD" = clean ]; then rm -rf "$CARGO_TARGET_DIR"; fi && \
    cargo build --release --locked --bin newport-agent && \
    mkdir /out && \
    case "$TARGETARCH" in amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) exit 1 ;; esac && \
    cp "$CARGO_TARGET_DIR/release/newport-agent" /out/newport-agent-$arch && \
    if [ "$NEWPORT_AGENT_BUILD" = clean ]; then rm -rf "$CARGO_TARGET_DIR"; fi && \
    file /out/newport-agent-$arch | grep -E 'static(ally linked|-pie linked)' && \
    /out/newport-agent-$arch --help

COPY tests/agent-runtime/private-directory.sh /checks/private-directory.sh
RUN adduser -D needle-check && chmod 0755 /tmp && \
    su needle-check -c 'sh /checks/private-directory.sh /out/newport-agent-*' && \
    chmod 1777 /tmp

RUN case "$TARGETARCH" in amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) exit 1 ;; esac && \
    /out/newport-agent-$arch product --json > /out/newport-agent-$arch.product.json

FROM scratch AS artifact
COPY --from=build /out/ /

FROM source AS discovery-build
ARG TARGETARCH
ARG NEWPORT_AGENT_VERSION
ENV NEWPORT_AGENT_VERSION=$NEWPORT_AGENT_VERSION
ENV CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
RUN apk add --no-cache jq
COPY crates/newport-agent/tests ./tests
COPY crates/newport-agent/benches ./benches
RUN --mount=type=cache,id=newport-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=newport-discovery-${TARGETARCH},target=/build/crates/newport-agent/target,sharing=locked \
    cargo build --locked --bin newport-agent && \
    cargo test --locked --lib --test discovery_helper --test discovery_monitor --test discovery_systemd --no-run --message-format=json > /tmp/discovery-artifacts.jsonl && \
    mkdir /out && cp target/debug/newport-agent /out/agent && \
    for spec in newport_agent:unit-tests discovery_helper:helper-tests discovery_monitor:monitor-tests discovery_systemd:systemd-tests; do \
        name=${spec%%:*}; output=${spec#*:}; \
        executable=$(jq -sr -e --arg name "$name" '[.[] | select(.reason == "compiler-artifact" and .target.name == $name and .profile.test == true and .executable != null) | .executable] | unique | if length == 1 then .[0] else error("Expected one test executable") end' /tmp/discovery-artifacts.jsonl) && \
        cp "$executable" "/out/$output" || exit 1; \
    done && \
    for executable in /out/*; do file "$executable" | grep -E 'static(ally linked|-pie linked)' || exit 1; done

FROM scratch AS discovery-artifact
COPY --from=discovery-build /out/ /
