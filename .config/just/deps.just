set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

deny:
    cargo deny check

machete:
    cargo machete --skip-target-dir $(find crates -maxdepth 1 -mindepth 1 -not -name kithara-workspace-hack | sort) tests xtask

shear:
    cargo shear --deny-warnings

hack:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo hack check --feature-powerset --no-dev-deps --depth 2 --workspace \
      --exclude kithara-play \
      --exclude kithara-app \
      --exclude kithara-fuzz \
      --exclude kithara-net
    # kithara-net rejects a backendless build with compile_error!;
    # client-apple builds only on Apple targets.
    cargo hack check --feature-powerset --no-dev-deps --depth 2 -p kithara-net \
      --at-least-one-of client-reqwest,client-wreq \
      --exclude-features client-apple

# Baseline is a revision here, not crates.io: the workspace is unpublished.
semver BASELINE="HEAD~1":
    cargo semver-checks check-release --workspace --exclude kithara-fuzz \
      --baseline-rev "{{ BASELINE }}"

geiger:
    cargo geiger --all-targets --all-dependencies --output-format Ascii

[positional-arguments]
manifest *ARGS:
    just _xtask manifest "$@"
