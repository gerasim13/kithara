set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

# Type-check the workspace.
[default]
workspace:
    cargo check --workspace

# Runs without the sccache wrapper: `clippy-driver` is not cached by it either
# way, and sccache aborts outright rather than fall back when it meets the
# incremental build that keeps this fast.
clippy:
    RUSTC_WRAPPER= CARGO_INCREMENTAL=1 cargo clippy --workspace -- -D warnings

[positional-arguments]
clippy-fix *ARGS:
    #!/bin/sh
    set -eu
    exec cargo clippy --fix --workspace --all-targets "$@" -- -D warnings

doc:
    cargo doc --workspace --no-deps --open
