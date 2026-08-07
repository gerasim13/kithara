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
    # A crate that forwards the HTTP backend choice cannot be built with none
    # of them: kithara-net says so with compile_error!. Those crates declare
    # the invariant; the rest keep the whole powerset, because --at-least-one-of
    # drops combinations from a crate that has none of the named features.
    # client-apple builds only on Apple targets.
    forwarding=$(grep -l '^client-reqwest' crates/*/Cargo.toml \
      | xargs -n1 dirname | xargs -n1 basename | sort)
    # kithara-stretch says the same thing about its time-stretch backends.
    stretch=$(grep -l '^stretch-signalsmith' crates/*/Cargo.toml \
      | xargs -n1 dirname | xargs -n1 basename | sort)
    skip=(kithara-play kithara-app kithara-fuzz)
    exclude=()
    for crate in "${skip[@]}"; do exclude+=(--exclude "$crate"); done
    for crate in $forwarding $stretch; do
      case " ${skip[*]} " in *" $crate "*) continue ;; esac
      exclude+=(--exclude "$crate")
    done
    cargo hack check --feature-powerset --no-dev-deps --depth 2 --workspace "${exclude[@]}"
    for crate in $forwarding; do
      case " ${skip[*]} " in *" $crate "*) continue ;; esac
      cargo hack check --feature-powerset --no-dev-deps --depth 2 -p "$crate" \
        --at-least-one-of client-reqwest,client-wreq \
        --at-least-one-of tls-rustls,tls-native \
        --exclude-features client-apple
    done
    for crate in $stretch; do
      case " ${skip[*]} " in *" $crate "*) continue ;; esac
      cargo hack check --feature-powerset --no-dev-deps --depth 2 -p "$crate" \
        --at-least-one-of stretch-signalsmith,stretch-bungee
    done

# Baseline is a revision here, not crates.io: the workspace is unpublished.
semver BASELINE="HEAD~1":
    cargo semver-checks check-release --workspace --exclude kithara-fuzz \
      --baseline-rev "{{ BASELINE }}"

geiger:
    cargo geiger --all-targets --all-dependencies --output-format Ascii

[positional-arguments]
manifest *ARGS:
    just _xtask manifest "$@"
