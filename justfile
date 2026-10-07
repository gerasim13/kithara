set shell := ["bash", "-euo", "pipefail", "-c"]

# Empty when absent: cargo reads that as "no wrapper", not as an error. Empty on
# Windows as well: sccache cannot spawn the compiler `ffmpeg-sys-next` asks for
# there, and the wrapper below is a POSIX script.
sccache := if os_family() == "windows" { "" } else { `command -v sccache 2>/dev/null || true` }

# sccache keys a Rust compile on every `CARGO_*` value, and Cargo builds at a
# lane slot's own path, which native build tools record. The wrapper runs
# sccache without the build directory, so that value no longer gives every
# dependency a copy per slot and job directory. The path still reaches a
# compile through `OUT_DIR` and through the proc macros a slot builds, so a
# crate that reads `OUT_DIR`, uses a proc macro, or depends on one that does is
# still kept once per slot.
rustc_wrapper := justfile_directory() / ".config/sccache/sccache"
export RUSTC_WRAPPER := if sccache == "" { "" } else { rustc_wrapper }

# A C compile's cache key includes the absolute paths in its preprocessor
# output, so the same C source built in two worktrees hashes twice and neither
# ever reads the other's entry. Each checkout names only its own root: the
# prefix is then stripped from that output, and what is left is the same
# workspace-relative path everywhere, so the keys coincide. A Rust compile is
# keyed on its raw working directory and every `CARGO_*` value, which this
# does not touch: a workspace crate is keyed per checkout regardless, and a
# dependency, which Cargo compiles inside its own home, shares its key across
# checkouts as far as the wrapper above lets it share across slots. This
# rewrites the cache key, not what the compiler is asked to compile.
export SCCACHE_BASEDIRS := if sccache == "" { "" } else { justfile_directory() }

# The cache was found sitting at exactly its ceiling - 60 GiB stored against a
# 60 GiB limit - which means it had been evicting entries it would be asked for
# again. A ceiling belongs with the rest of the build's configuration rather
# than in whichever shell happened to start the server, so it is named here.
# The server reads this once, when it starts, so a machine whose server is
# already up keeps its old limit until `sccache --stop-server`.
export SCCACHE_CACHE_SIZE := if sccache == "" { "" } else { "200G" }

# Where this machine keeps the FFmpeg line the workspace binds to. `ffmpeg-next`
# generates its bindings from the headers pkg-config finds, so a host whose
# unversioned FFmpeg has moved on has to reach the keg-only formula
# `.config/ci-pins.toml` installs before it. Both halves belong to the machine
# and are read off it: the line from the pins, the prefix from where `brew`
# itself sits. Nothing here runs the package manager, because `just` evaluates
# every variable before it knows which recipe was asked for, so this is on the
# clock of every invocation including the nested ones a test drives. A keg the
# machine does not have leaves this empty and its own search path stands alone.
# `xtask` cannot own this: it is itself a cargo build that would need the answer
# before it could run.
export PKG_CONFIG_PATH := ```
    formula=$(grep -om1 'ffmpeg@[0-9][0-9]*' .config/ci-pins.toml 2>/dev/null || true)
    brew=$(command -v brew || true)
    keg="${brew%/bin/brew}/opt/$formula/lib/pkgconfig"
    [ -n "$brew" ] && [ -n "$formula" ] && [ -d "$keg" ] || keg=
    printf '%s' "$keg:${PKG_CONFIG_PATH:-}" | sed 's/^://; s/:$//'
```

# Whether Cargo fetches a git dependency with the system git rather than its
# own client. The system git is much faster on a large history - `btls-sys`
# carries `boringssl`, measured at 25.6 minutes on the Apple host (GitLab job
# 9811155) - but it fetches with whatever credentials the machine has, and a
# Linux container has none: GitHub answered its anonymous request with a
# challenge and git, having no terminal, failed the job outright. Cargo's own
# client asks anonymously and never prompts. So the faster path is taken only
# on the hosts it was measured on and where it works.
export CARGO_NET_GIT_FETCH_WITH_CLI := if os() == "macos" { "true" } else { "false" }

# sccache refuses incremental compilations, so a wrapper without this is never
# hit. `check clippy` opts back in on a workstation, where the dependencies are
# already built and incremental turns 15s into 2.4s, and leaves the shared cache
# alone in CI, where nothing is already built.
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

# Desktop launches share the build environment above with checks and CI.
[positional-arguments]
run *ARGS: _desktop-ready
    @exec cargo run --locked -p kithara-app --release --bin kithara "$@"

[positional-arguments]
gallery *ARGS: _desktop-ready
    @exec cargo run --locked -p kithara-ui-gallery --release --bin gallery "$@"

# Cargo reads the shared config even when Git honors config.worktree. Repair
# only a primary checkout that Git itself recognizes as a non-bare worktree.
[private]
_desktop-ready:
    @if [[ -d "$PWD/.git" ]] && [[ "$(git rev-parse --absolute-git-dir)" = "$PWD/.git" ]] && [[ "$(git config --local --bool core.bare || true)" = true ]] && [[ "$(git rev-parse --is-bare-repository)" = false ]] && [[ "$(git rev-parse --show-toplevel)" = "$PWD" ]]; then git config --local core.bare false; printf 'Repaired contradictory core.bare setting for this checkout.\n' >&2; fi

[no-exit-message]
[positional-arguments]
_xtask *ARGS:
    @if [[ -z "${KITHARA_CI_CACHE_ROOT:-}" ]]; then exec just _xtask-unleased "$@"; fi; trust="${KITHARA_CACHE_TRUST:?a CI cache root needs the trust namespace it belongs to}"; if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then build_target="$CARGO_TARGET_DIR"; else build_target=$(just _xtask-self-target) || exit $?; fi; mkdir -p "$build_target"; export CARGO_HOME="$KITHARA_CI_CACHE_ROOT/$trust/$(rustc --print cfg | sed -n 's/^target_os="\(.*\)"$/\1/p')-$(rustc --print cfg | sed -n 's/^target_arch="\(.*\)"$/\1/p')/cargo"; mkdir -p "$CARGO_HOME"; helper="${TMPDIR:-/tmp}/kithara-target-lease-${CI_JOB_ID:-$$}-$$"; rustc --edition=2024 "$PWD/xtask/bootstrap_lease.rs" -o "$helper"; exec "$helper" "$build_target/.kithara-job-lease" just _xtask-unleased "$@"

[no-exit-message]
[positional-arguments]
[private]
_xtask-unleased *ARGS: _xtask-ready
    @exec just _xtask-cached strict "$@"

[no-exit-message]
_xtask-refresh:
    @target=$(just _xtask-self-target) || exit $?; export CARGO_TARGET_DIR="$target"; \
    if just _xtask-cached strict self-cache probe </dev/null >/dev/null 2>&1; then \
      if just _xtask-build-env just _xtask-cached strict self-cache refresh --force </dev/null; then exit 0; fi; \
      printf 'warning: cached xtask self-cache maintenance failed; rebuilding from source\n' >&2; \
    fi; exec just _xtask-bootstrap --force </dev/null

[no-exit-message]
[private]
_xtask-ready:
    @if ! just _xtask-cached strict self-cache probe </dev/null >/dev/null 2>&1; then exec just _xtask-bootstrap </dev/null >/dev/null; fi; \
    if state=$(just _xtask-cached strict self-cache status </dev/null); then \
      case "$state" in current) exit 0 ;; stale) ;; *) printf 'error: invalid xtask cache status: %s\n' "$state" >&2; exit 1 ;; esac; \
      if bootstrap_probe=$(just _xtask-cached strict self-cache probe --bootstrap </dev/null 2>&1); then :; \
      else probe_status=$?; \
        if [[ "$probe_status" -eq 2 ]]; then exec just _xtask-bootstrap </dev/null >/dev/null; fi; \
        printf '%s\n' "$bootstrap_probe" >&2; exit "$probe_status"; \
      fi; \
    fi; \
    target=$(just _xtask-self-target) || exit $?; CARGO_TARGET_DIR="$target" exec just _xtask-build-env just _xtask-cached strict self-cache refresh </dev/null >/dev/null

# An older cached binary can still own the first refresh of new source. It
# reads CARGO directly, so the transport must give it the pinned executable as
# well as the checksum mode that binary's Cargo inherits.
[no-exit-message]
[positional-arguments]
[private]
_xtask-build-env *ARGS:
    @toolchain=$(sed -n 's/^nightly_toolchain = "\([^"]*\)"$/\1/p' .config/ci-pins.toml); [[ "$toolchain" = nightly-* && "$toolchain" != *$'\n'* ]] || { printf 'error: invalid xtask bootstrap nightly pin\n' >&2; exit 1; }; cargo=$(rustup which --toolchain "$toolchain" cargo) || exit $?; exec env -u XTASK_SELF_CACHE_CARGO -u RUSTC -u RUSTDOC -u CARGO_BUILD_RUSTC -u CARGO_BUILD_RUSTDOC CARGO="$cargo" RUSTUP_TOOLCHAIN="$toolchain" CARGO_UNSTABLE_CHECKSUM_FRESHNESS=true rustup run "$toolchain" "$@"

# Where the self-cache builds: the checkout's own directory locally, and on
# CI the bootstrap namespace the host cleaner owns, never a lane's directory.
[no-exit-message]
[private]
_xtask-self-target:
    @if [[ -z "${KITHARA_CI_CACHE_ROOT:-}" ]]; then printf '%s\n' "$PWD/target/xtask-self-cache"; exit 0; fi; trust="${KITHARA_CACHE_TRUST:?a CI cache root needs the trust namespace it belongs to}"; owner="${CI_CONCURRENT_ID:-local}"; case "$owner" in *[!A-Za-z0-9_.-]*) printf 'error: invalid xtask bootstrap cache owner: %s\n' "$owner" >&2; exit 1 ;; esac; printf '%s\n' "$KITHARA_CI_CACHE_ROOT/bootstrap/$trust/target-$(uname -s)-$(uname -m)-$owner"

# The one build with no caches of its own. Their variables are normally produced
# by `CiEnvironment`, inside the binary this build is compiling. Its target and
# compiler cache stay in the bootstrap namespace the host cleaner owns, but its
# `CARGO_HOME` is the job's own: this build is what fetches the git
# dependencies, and giving it a home of its own made the lane fetch the same
# submodules a second time minutes later. `_xtask` names that home, and
# `CiEnvironment` refuses to disagree with it. A daemon keeps the cache
# directory it started with, so an executor-provided socket gets a distinct
# bootstrap endpoint when the directory changes.
[no-exit-message]
[positional-arguments]
[private]
_xtask-bootstrap *ARGS:
    @target=$(just _xtask-self-target) || exit $?; if [[ -n "${KITHARA_CI_CACHE_ROOT:-}" ]]; then root="${target%/*}"; export SCCACHE_DIR="$root/sccache"; if [[ -n "${SCCACHE_SERVER_UDS:-}" ]]; then export SCCACHE_SERVER_UDS="/tmp/kithara-xtask-${root##*/}-${target##*/target-}.sock"; fi; fi; exec env CARGO_TARGET_DIR="$target" just _xtask-build-env cargo run --locked --manifest-path "$PWD/Cargo.toml" -p xtask --bin xtask -- self-cache bootstrap "$@"

# The pointer to the active generation lives beside the generations it names,
# inside the Git directory. A CI runner cleans the working tree before every
# job, which used to remove a pointer kept there while leaving the generations
# themselves intact: the cached binary was present and unreachable, so every
# job rebuilt it from source and paid the full dependency fetch to do so.
#
# The Git directory is resolved by reading `.git` rather than by running Git,
# because this transport must stay free of both Cargo and Git - a test asserts
# it. A linked worktree spells `.git` as a file naming the real directory.
#
# A generation path is accepted as absolute or as a drive letter, and the
# drive letter is read two characters at a time rather than matched against a
# pattern holding a backslash: the bash the Windows guest runs mangles one
# inside a bracket expression, so `[A-Za-z]:[\\/]*` matched a forward slash
# alone there and refused every generation the guest published. What separates
# the drive from the rest of the path is left to the executable test below,
# which asks the filesystem instead of a pattern.
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
      if [ -d "$PWD/.git" ]; then git_dir="$PWD/.git"; \
      elif [ -f "$PWD/.git" ]; then \
        git_dir=$(sed -n 's/^gitdir: //p' "$PWD/.git") || unavailable; \
        [ -n "$git_dir" ] || unavailable; \
        case "$git_dir" in /*) ;; *) git_dir="$PWD/$git_dir" ;; esac; \
      else unavailable; fi; \
      pointer="$git_dir/xtask-cache/active"; \
      [ -f "$pointer" ] && [ ! -L "$pointer" ] && [ -r "$pointer" ] || unavailable; \
      size=$(wc -c < "$pointer") || unavailable; \
      { [ "$size" -ge 1 ] 2>/dev/null && [ "$size" -le 4096 ] 2>/dev/null; } || unavailable; \
      generation=; extra=; \
      if ! { IFS= read -r generation && ! IFS= read -r extra && [ -z "$extra" ]; } < "$pointer"; then \
        unavailable; \
      fi; \
      system=$(uname -s) || unavailable; \
      case "$system" in \
        MINGW*|MSYS*|CYGWIN*) windows=1; suffix=.exe ;; \
        *) windows=0; suffix= ;; \
      esac; \
      case "$generation" in \
        /*) ;; \
        *) case "$(printf %.2s "$generation")" in \
             [A-Za-z]:) [ "$windows" -eq 1 ] || unavailable ;; \
             *) unavailable ;; \
           esac ;; \
      esac; \
      binary="$generation/xtask$suffix"; \
      [ -f "$binary" ] && [ ! -L "$binary" ] && [ -x "$binary" ] || unavailable; \
      if [ "$mode" = optional ]; then \
        "$binary" self-cache probe --config </dev/null >/dev/null 2>&1 || unavailable; \
      fi; \
      exec "$binary" "$@"

[no-exit-message]
_agent-hook: (_xtask-cached "optional" "agent-hook")
