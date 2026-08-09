set shell := ["bash", "-euo", "pipefail", "-c"]

# Clippy runs `clippy-driver` in place of `rustc`, so it compiles the whole
# graph before it can lint one line, and it does so into its own profile
# directory — a suite run in `test-release` leaves it nothing to reuse. The CI
# image has kept `sccache` for exactly this since it was built; a workstation
# had no equivalent, so every branch switch bought a full rebuild before the
# pre-commit hook could say anything. Empty when sccache is absent, which cargo
# reads as "no wrapper" rather than as an error.
sccache := `command -v sccache 2>/dev/null || true`

export RUSTC_WRAPPER := sccache

# sccache declines to cache an incremental compilation and cargo leaves
# incremental on, so a wrapper set without this is installed, enabled, and never
# hit — measured here as ten compile requests and zero of them cached. Left
# alone when there is no wrapper, so a workstation without sccache keeps the
# incremental rebuilds it does benefit from.
export CARGO_INCREMENTAL := if sccache == "" { "" } else { "0" }

mod fmt ".config/just/fmt.just"
mod check ".config/just/check.just"
mod lint ".config/just/lint.just"
mod test ".config/just/test.just"
mod quality ".config/just/quality.just"
mod deps ".config/just/deps.just"
mod arch ".config/just/arch.just"
mod perf ".config/just/perf.just"
mod platform ".config/just/platform.just"
mod release ".config/just/release.just"
mod ci ".config/just/ci.just"
mod tooling ".config/just/tooling.just"

# Human-facing overview. Agents use the exact paths documented in AGENTS.md.
[default]
help:
    @just --list

[no-exit-message]
[positional-arguments]
_xtask *ARGS: _xtask-ready
    @exec just _xtask-cached strict "$@"

[no-exit-message]
_xtask-refresh:
    @if just _xtask-cached strict self-cache probe </dev/null >/dev/null 2>&1; then \
      if just _xtask-cached strict self-cache refresh --force </dev/null; then exit 0; fi; \
      printf 'warning: cached xtask self-cache maintenance failed; rebuilding from source\n' >&2; \
    fi; exec just _xtask-bootstrap --force </dev/null

[no-exit-message]
[private]
_xtask-ready:
    @if ! just _xtask-cached strict self-cache probe </dev/null >/dev/null 2>&1; then exec just _xtask-bootstrap </dev/null >/dev/null; fi; state=$(just _xtask-cached strict self-cache status </dev/null) || exit $?; case "$state" in current) ;; stale) exec just _xtask-cached strict self-cache refresh </dev/null >/dev/null ;; *) printf 'error: invalid xtask cache status: %s\n' "$state" >&2; exit 1 ;; esac

[no-exit-message]
[positional-arguments]
[private]
_xtask-bootstrap *ARGS:
    @exec env CARGO_TARGET_DIR="$PWD/target/xtask-self-cache" cargo run --locked --manifest-path "$PWD/Cargo.toml" -p xtask --bin xtask -- self-cache bootstrap "$@"

[no-exit-message]
[positional-arguments]
[private]
_xtask-cached MODE *ARGS:
    @set -eu; \
      mode=$1; shift; \
      case "$mode" in \
        strict|optional) ;; \
        *) printf 'error: invalid xtask transport mode: %s\n' "$mode" >&2; exit 2 ;; \
      esac; \
      unavailable() { \
        if [ "$mode" = optional ]; then \
          printf 'warning: cached xtask transport is unavailable; run just tooling xtask --help to install it\n' >&2; \
          exit 0; \
        fi; \
        printf 'error: cached xtask transport is unavailable\n' >&2; \
        exit 1; \
      }; \
      pointer="$PWD/xtask/.xtask-cache"; \
      [ -f "$pointer" ] && [ ! -L "$pointer" ] && [ -r "$pointer" ] || unavailable; \
      size=$(wc -c < "$pointer") || unavailable; \
      { [ "$size" -ge 1 ] 2>/dev/null && [ "$size" -le 4096 ] 2>/dev/null; } || unavailable; \
      generation=; extra=; \
      if ! { IFS= read -r generation && ! IFS= read -r extra && [ -z "$extra" ]; } < "$pointer"; then \
        unavailable; \
      fi; \
      case "$generation" in \
        /*) ;; \
        *) unavailable ;; \
      esac; \
      binary="$generation/xtask"; \
      [ -f "$binary" ] && [ ! -L "$binary" ] && [ -x "$binary" ] || unavailable; \
      if [ "$mode" = optional ]; then \
        "$binary" self-cache probe </dev/null >/dev/null 2>&1 || unavailable; \
      fi; \
      exec "$binary" "$@"

[no-exit-message]
_agent-hook: (_xtask-cached "optional" "agent-hook")
