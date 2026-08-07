set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

deny:
    cargo deny check

machete:
    cargo machete --skip-target-dir $(find crates -maxdepth 1 -mindepth 1 -not -name kithara-workspace-hack | sort) tests xtask

shear:
    cargo shear --deny-warnings

hack:
    just _xtask powerset --no-dev-deps

# Baseline is a revision here, not crates.io: the workspace is unpublished.
semver BASELINE="HEAD~1":
    cargo semver-checks check-release --workspace --exclude kithara-fuzz \
      --baseline-rev "{{ BASELINE }}"

geiger:
    cargo geiger --all-targets --all-dependencies --output-format Ascii

[positional-arguments]
manifest *ARGS:
    just _xtask manifest "$@"
