ARG CACHE_IMAGE
ARG RUST_VERSION
ARG RUST_BASE_DIGEST
FROM ${CACHE_IMAGE} AS cache
FROM rust:${RUST_VERSION}-bookworm@sha256:${RUST_BASE_DIGEST}

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY tests tests
COPY xtask xtask
RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --locked -p xtask --bin xtask \
    && cp target/debug/xtask /usr/local/bin/xtask
COPY --from=cache /usr/bin/minio /usr/bin/mc /usr/bin/docker-entrypoint.sh /usr/bin/
WORKDIR /
ENTRYPOINT ["/usr/bin/docker-entrypoint.sh"]
CMD ["server", "/data"]
