ARG RUST_BASE_DIGEST
ARG RUST_VERSION
FROM rust:${RUST_VERSION}-bookworm@sha256:${RUST_BASE_DIGEST}

ARG AST_GREP_VERSION
ARG CARGO_DENY_VERSION
ARG CARGO_HACK_VERSION
ARG CARGO_LLVM_COV_VERSION
ARG CARGO_MACHETE_VERSION
ARG CARGO_MUTANTS_VERSION
ARG CARGO_NEXTEST_VERSION
ARG CARGO_SEMVER_CHECKS_VERSION
ARG CARGO_SHEAR_VERSION
ARG CARGO_SORT_VERSION
ARG CMAKE_AMD64_SHA256
ARG CMAKE_ARM64_SHA256
ARG CMAKE_VERSION
ARG GECKODRIVER_AMD64_SHA256
ARG GECKODRIVER_ARM64_SHA256
ARG GECKODRIVER_VERSION
ARG GITLEAKS_AMD64_SHA256
ARG GITLEAKS_ARM64_SHA256
ARG GITLEAKS_VERSION
ARG JUST_VERSION
ARG MD_FORMATTER_VERSION
ARG MSRV_TOOLCHAIN
ARG NIGHTLY_TOOLCHAIN
ARG SCCACHE_VERSION
ARG SIMILARITY_RS_VERSION
ARG TAPLO_CLI_VERSION
ARG TIDY_JSON_VERSION
ARG TRUNK_VERSION
ARG TYPOS_CLI_VERSION
ARG WASM_BINDGEN_CLI_VERSION
ARG WASM_PACK_VERSION
ARG WASM_SLIM_VERSION

ENV KITHARA_MSRV_TOOLCHAIN=${MSRV_TOOLCHAIN}
ENV KITHARA_NIGHTLY_TOOLCHAIN=${NIGHTLY_TOOLCHAIN}
ENV WASM_SLIM_TOOLCHAIN=${NIGHTLY_TOOLCHAIN}

# Firefox alone is 76 megabytes, which does not fit the two-minute default
# transfer timeout on a slow mirror, and a build that reaches that point has
# already spent minutes to be told the connection failed.
RUN apt-get update && apt-get install -y --no-install-recommends \
    -o Acquire::Retries=5 -o Acquire::http::Timeout=600 \
    ca-certificates chromium chromium-driver curl ffmpeg firefox-esr git \
    clang libclang-dev lld pkg-config \
    bubblewrap socat ripgrep nodejs npm \
    libasound2-dev libdbus-1-dev libssl-dev \
    libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev \
    libavutil-dev libswresample-dev libswscale-dev libpostproc-dev \
    && rm -rf /var/lib/apt/lists/*

# Debian ships CMake 3.25, and a vendored native dependency requires 3.30 or
# newer, so `cargo check` could not build the workspace here at all. The 3.31
# series is deliberate: CMake 4 refuses any project that asks for a minimum
# below 3.5, which several vendored trees still do.
RUN case "$(dpkg --print-architecture)" in \
      amd64) slice=x86_64; sum="${CMAKE_AMD64_SHA256}" ;; \
      arm64) slice=aarch64; sum="${CMAKE_ARM64_SHA256}" ;; \
      *) echo "unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac \
 && curl -fsSL \
      -o /tmp/cmake.tar.gz \
      "https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/cmake-${CMAKE_VERSION}-linux-${slice}.tar.gz" \
 && echo "${sum}  /tmp/cmake.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/cmake.tar.gz -C /usr/local --strip-components=1 \
 && rm /tmp/cmake.tar.gz \
 && cmake --version

RUN case "$(dpkg --print-architecture)" in \
      amd64) slice=linux64; sum="${GECKODRIVER_AMD64_SHA256}" ;; \
      arm64) slice=linux-aarch64; sum="${GECKODRIVER_ARM64_SHA256}" ;; \
      *) echo "unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac \
 && curl -fsSL \
      -o /tmp/geckodriver.tar.gz \
      "https://github.com/mozilla/geckodriver/releases/download/v${GECKODRIVER_VERSION}/geckodriver-v${GECKODRIVER_VERSION}-${slice}.tar.gz" \
 && echo "${sum}  /tmp/geckodriver.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/geckodriver.tar.gz -C /usr/local/bin geckodriver \
 && rm /tmp/geckodriver.tar.gz \
 && ln -s /usr/bin/firefox-esr /usr/local/bin/firefox

RUN case "$(dpkg --print-architecture)" in \
      amd64) slice=x64; sum="${GITLEAKS_AMD64_SHA256}" ;; \
      arm64) slice=arm64; sum="${GITLEAKS_ARM64_SHA256}" ;; \
      *) echo "unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac \
 && curl -fsSL \
      -o /tmp/gitleaks.tar.gz \
      "https://github.com/gitleaks/gitleaks/releases/download/v${GITLEAKS_VERSION}/gitleaks_${GITLEAKS_VERSION}_linux_${slice}.tar.gz" \
 && echo "${sum}  /tmp/gitleaks.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/gitleaks.tar.gz -C /usr/local/bin gitleaks \
 && rm /tmp/gitleaks.tar.gz

# `rust-src` on the default toolchain too: the workspace builds the standard
# library from source for some targets, and `cargo-semver-checks` inherits that
# when it runs rustdoc — without the sources every crate it documents fails
# before it can compare a single signature.
RUN rustup component add clippy llvm-tools-preview rust-src rustfmt \
 && rustup toolchain install "${NIGHTLY_TOOLCHAIN}" \
      --profile minimal \
      --component miri \
      --component rust-src \
      --component rustfmt \
 && rustup toolchain install "${MSRV_TOOLCHAIN}" --profile minimal \
 && rustup target add wasm32-unknown-unknown \
 && rustup target add wasm32-unknown-unknown --toolchain "${NIGHTLY_TOOLCHAIN}" \
 && host="$(rustc -vV | sed -n 's/^host: //p')" \
 && ln -s "${RUSTUP_HOME}/toolchains/${NIGHTLY_TOOLCHAIN}-${host}" \
      "${RUSTUP_HOME}/toolchains/nightly-${host}"

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/tmp/cargo-install-target \
    CARGO_TARGET_DIR=/tmp/cargo-install-target \
    cargo install --locked --version "${AST_GREP_VERSION}" ast-grep \
 && cargo install --locked --version "${CARGO_DENY_VERSION}" cargo-deny \
 && cargo install --locked --version "${CARGO_HACK_VERSION}" cargo-hack \
 && cargo install --locked --version "${CARGO_LLVM_COV_VERSION}" cargo-llvm-cov \
 && cargo install --locked --version "${CARGO_MACHETE_VERSION}" cargo-machete \
 && cargo install --locked --version "${CARGO_MUTANTS_VERSION}" cargo-mutants \
 && cargo install --locked --version "${CARGO_NEXTEST_VERSION}" cargo-nextest \
 && cargo install --locked --version "${CARGO_SEMVER_CHECKS_VERSION}" cargo-semver-checks \
 && cargo install --locked --version "${CARGO_SHEAR_VERSION}" cargo-shear \
 && cargo install --locked --version "${CARGO_SORT_VERSION}" cargo-sort \
 && cargo install --locked --version "${JUST_VERSION}" just \
 && cargo install --locked --version "${MD_FORMATTER_VERSION}" md-formatter \
 && cargo install --locked --version "${SCCACHE_VERSION}" sccache \
 && cargo install --locked --version "${SIMILARITY_RS_VERSION}" similarity-rs \
 && cargo install --locked --version "${TAPLO_CLI_VERSION}" taplo-cli \
 && cargo install --locked --version "${TIDY_JSON_VERSION}" tidy-json \
 && cargo install --locked --version "${TRUNK_VERSION}" trunk \
 && cargo install --locked --version "${TYPOS_CLI_VERSION}" typos-cli \
 && cargo install --locked --version "${WASM_BINDGEN_CLI_VERSION}" wasm-bindgen-cli \
 && cargo install --locked --version "${WASM_PACK_VERSION}" wasm-pack \
 && cargo install --locked --version "${WASM_SLIM_VERSION}" wasm-slim
