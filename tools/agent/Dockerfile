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

FROM checked-runtime AS build
ARG TARGETARCH
COPY tools/agent/Cargo.toml tools/agent/Cargo.lock ./
COPY tools/agent/src ./src
RUN --mount=type=cache,id=newport-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=newport-agent-${TARGETARCH},target=/build/target,sharing=locked \
    cargo build --release --locked --bin newport-agent && \
    mkdir /out && \
    case "$TARGETARCH" in amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) exit 1 ;; esac && \
    cp target/release/newport-agent /out/newport-agent-$arch && \
    file /out/newport-agent-$arch | grep -E 'static(ally linked|-pie linked)' && \
    /out/newport-agent-$arch --help

FROM scratch AS artifact
COPY --from=build /out/ /
