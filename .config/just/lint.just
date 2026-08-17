set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

# Run all workspace architecture, style, and idiom ratchets.
[default]
[positional-arguments]
all *ARGS:
    just _xtask lint "$@"

[positional-arguments]
arch *ARGS:
    just _xtask lint arch "$@"

[positional-arguments]
style *ARGS:
    just _xtask lint style "$@"

[positional-arguments]
idioms *ARGS:
    just _xtask lint idioms "$@"

_shared:
    just fmt check
    just check clippy
    just lint ast-grep
    just lint typos
    just lint arch

# Fast local chain: formatting, Clippy, ast-grep, typos, and arch ratchet.
fast:
    just lint _shared
    just lint idioms --check derivable_getter --check derivable_delegation

# Full CI chain: fast checks, xtask tests, quality scans, style, and idioms.
# It runs every idiom check below, so it takes the shared part directly rather
# than through `fast`, whose two-check pass it would then repeat.
full:
    just lint _shared
    cargo test -p xtask
    just quality unimock-check
    just quality rstest-audit
    just quality trait-mock-audit
    just quality trait-mock-exceptions
    just lint style
    just lint idioms

[positional-arguments]
ast-grep *ARGS:
    just _xtask ast-grep "$@"

[positional-arguments]
audit-clippy *ARGS:
    just _xtask audit-clippy "$@"

[positional-arguments]
typos *ARGS:
    just _xtask typos "$@"

[positional-arguments]
similarity *ARGS:
    just _xtask similarity "$@"

[positional-arguments]
deadlock *ARGS:
    exec cargo "+${KITHARA_LOCKBUD_TOOLCHAIN:-nightly}" lockbud -k deadlock --workspace "$@"

unused-pub:
    cargo workspace-unused-pub
