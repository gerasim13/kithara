ARG CACHE_SERVER_IMAGE
ARG CACHE_CLIENT_IMAGE
ARG RUST_VERSION
ARG RUST_BASE_DIGEST
FROM ${CACHE_SERVER_IMAGE} AS server
FROM ${CACHE_CLIENT_IMAGE} AS client
FROM rust:${RUST_VERSION}-bookworm@sha256:${RUST_BASE_DIGEST}

# xtask's proc-macros pull the workspace's audio stack into the host build,
# and its ALSA bindings compile against the system headers.
RUN apt-get update && apt-get install -y --no-install-recommends \
    -o Acquire::Retries=5 libasound2-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
# The workspace's crates.io patches live here; without them the lock names
# sources the build cannot find.
COPY .cargo/config.toml .cargo/config.toml
COPY crates crates
COPY tests tests
COPY xtask xtask
RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --locked -p xtask --bin xtask \
    && cp target/debug/xtask /usr/local/bin/xtask
COPY --from=server /usr/bin/rustfs /usr/bin/rustfs
COPY --from=client /usr/bin/rc /usr/bin/rc
WORKDIR /
ENTRYPOINT ["/usr/bin/rustfs"]
CMD ["server", "/data"]
