set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

# Publish public crates to crates.io in dependency order.
[positional-arguments]
crates *ARGS:
    just _xtask publish "$@"

# Build and cache framework, docs, and WASM release artifacts.
artifacts VERSION:
    #!/bin/sh
    set -eu
    version=$1
    just release changelog "$version"
    just platform apple doc
    just _xtask wasm build --profile release
    exec just _xtask release prepare "$version"

# Render CHANGELOG.md from commit subjects; a VERSION names the unreleased
# commits as that release.
[positional-arguments]
changelog *VERSION:
    git-cliff --config .config/cliff.toml ${1:+--tag "v$1"} \
      --output CHANGELOG.md v0.0.1-alpha4..HEAD

# Upload cached framework and docs assets.
publish REF="HEAD":
    exec just _xtask release publish --ref "$1"

# Deploy the cached WASM bundle to GitHub Pages.
pages REF="HEAD":
    exec just _xtask release pages --ref "$1"
