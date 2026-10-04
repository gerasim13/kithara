#!/usr/bin/env bash
set -euo pipefail

revision="$1"
install_root="$2"
cargo="${3:-cargo}"
script_dir="$(cd "$(dirname "$0")" && pwd)"
source_dir="$(mktemp -d)"
trap 'rm -rf "$source_dir"' EXIT

git -C "$source_dir" init --quiet
git -C "$source_dir" remote add origin https://github.com/geiger-rs/cargo-geiger.git
git -C "$source_dir" fetch --quiet --depth=1 origin "$revision"
git -C "$source_dir" checkout --quiet --detach FETCH_HEAD
git -C "$source_dir" apply "$script_dir/cargo-geiger.patch"
cd "$source_dir"
"$cargo" test --manifest-path "$source_dir/Cargo.toml" --locked --release     -p cargo-geiger --test compiled_inputs
"$cargo" install --force --root "$install_root" --locked --path "$source_dir/cargo-geiger"
