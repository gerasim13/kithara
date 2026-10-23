set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

# Format Rust, Cargo manifests, TOML, JSON, and Markdown.
[default]
all:
    just _xtask format --allow-dirty
    cargo +"${KITHARA_NIGHTLY_TOOLCHAIN:-nightly}" fmt --all --manifest-path crates/kithara-test-utils/tests/fixtures/external-runtime/Cargo.toml
    cargo sort --grouped --config .config/tomlfmt.toml crates/kithara-test-utils/tests/fixtures/external-runtime

# Check all supported formatting without modifying files.
check:
    just _xtask format --check
    cargo +"${KITHARA_NIGHTLY_TOOLCHAIN:-nightly}" fmt --all --check --manifest-path crates/kithara-test-utils/tests/fixtures/external-runtime/Cargo.toml
    cargo sort --grouped --check --check-format --config .config/tomlfmt.toml crates/kithara-test-utils/tests/fixtures/external-runtime

# Sort every workspace Cargo.toml and keep kithara crates first.
cargo:
    just _xtask format --only manifest --allow-dirty

cargo-check:
    just _xtask format --only manifest --check

toml:
    just _xtask format --only toml --allow-dirty

toml-check:
    just _xtask format --only toml --check

json:
    just _xtask format --only json --allow-dirty

json-check:
    just _xtask format --only json --check

markdown:
    just _xtask format --only markdown --allow-dirty

markdown-check:
    just _xtask format --only markdown --check
