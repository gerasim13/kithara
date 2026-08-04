set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

# Run workspace tests via the xtask/nextest harness. Use `just test run ...`

# when passing lane selectors, package filters, or nextest expressions.
[default]
[positional-arguments]
run *ARGS:
    just _xtask test "$@"

all:
    just test
    just test run --lane=doc

[positional-arguments]
e2e-resamplers *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if (( $# == 0 )); then
      set -- -E 'test(track_plays_end_to_end_silvercomet)' --test-threads=4
    fi
    just test run --lane=e2e "$@"
    just test run --lane=e2e-fused "$@"
    just test run --lane=e2e-glide "$@"

# Cobertura and LCOV reports from one instrumented test run.
coverage:
    #!/usr/bin/env bash
    set -uo pipefail
    OUTPUT_DIR="${COVERAGE_OUTPUT_DIR:-./coverage}"
    COVERAGE_MIN="${COVERAGE_MIN:-80}"
    IGNORE_REGEX='(tests/|examples/|benches/)'
    COBERTURA="$OUTPUT_DIR/cobertura.xml"
    LCOV="$OUTPUT_DIR/lcov.info"
    mkdir -p "$OUTPUT_DIR"
    rm -f "$COBERTURA" "$LCOV"
    cargo llvm-cov clean --workspace
    test_rc=0
    cargo llvm-cov nextest --no-report \
      --workspace --exclude kithara-fuzz \
      --cargo-profile test-release || test_rc=$?
    report_rc=0
    cargo llvm-cov report \
      --profile test-release \
      --cobertura \
      --output-path "$COBERTURA" \
      --ignore-filename-regex "$IGNORE_REGEX" || report_rc=$?
    cargo llvm-cov report \
      --profile test-release \
      --lcov \
      --output-path "$LCOV" \
      --ignore-filename-regex "$IGNORE_REGEX" || report_rc=$?
    cargo llvm-cov report \
      --profile test-release \
      --summary-only \
      --ignore-filename-regex "$IGNORE_REGEX" \
      --fail-under-lines "$COVERAGE_MIN" || report_rc=$?
    echo "==> coverage report written to $COBERTURA"
    echo "==> LCOV report written to $LCOV"
    echo "==> line coverage threshold: $COVERAGE_MIN%"
    if (( test_rc != 0 )); then
      exit "$test_rc"
    fi
    exit "$report_rc"

# Show lines not executed by any test.
uncovered:
    cargo llvm-cov clean --workspace
    cargo llvm-cov nextest --no-report --workspace --exclude kithara-fuzz \
      --features kithara-audio/rodio \
      --cargo-profile test-release --no-fail-fast || true
    cargo llvm-cov nextest --no-report \
      -p kithara-integration-tests --features e2e \
      --cargo-profile test-release \
      --test suite_e2e --run-ignored all --no-fail-fast || true
    cargo llvm-cov report \
      --ignore-filename-regex '(tests/|examples/|benches/)' \
      --show-missing-lines

# Focused mutation testing. Only suites declared in mutation-suites.toml are accepted.
[positional-arguments]
mutants *ARGS:
    just _xtask mutants "$@"

[positional-arguments]
loom-checkpoint FILE *ARGS:
    #!/bin/sh
    set -eu
    file=$1
    shift
    LOOM_CHECKPOINT_FILE="$file" exec just test run --loom=on "$@"

[positional-arguments]
loom-isolate FILE *ARGS:
    #!/bin/sh
    set -eu
    file=$1
    shift
    LOOM_CHECKPOINT_INTERVAL=1 LOOM_CHECKPOINT_FILE="$file" exec just test run --loom=on "$@"

[positional-arguments]
loom-debug FILE *ARGS:
    #!/bin/sh
    set -eu
    file=$1
    shift
    LOOM_LOG=trace LOOM_LOCATION=1 LOOM_CHECKPOINT_INTERVAL=1 LOOM_CHECKPOINT_FILE="$file" exec just test run --loom=on --no-capture "$@"

# The sanitizer needs `-Zbuild-std`, which needs the `rust-src` component. The
# repository pins a nightly and installs the component into that one; plain
# `nightly` is whatever the host last downloaded and usually has no `rust-src`.
_rtsan SUITE FILTER:
    #!/usr/bin/env bash
    set -euo pipefail
    target="$(rustc -vV | sed -n 's/^host: //p')"
    toolchain="${KITHARA_NIGHTLY_TOOLCHAIN:-nightly}"
    RUSTFLAGS="-Zsanitizer=realtime --cfg rtsan" \
    RUSTDOCFLAGS="-Zsanitizer=realtime --cfg rtsan" \
        cargo "+$toolchain" test -p kithara-integration-tests --test {{ SUITE }} \
        --features apple-fused-src \
        --target "$target" -- --nocapture {{ FILTER }}

rtsan-async FILTER="no_block":
    #!/usr/bin/env bash
    set -euo pipefail
    target="$(rustc -vV | sed -n 's/^host: //p')"
    toolchain="${KITHARA_NIGHTLY_TOOLCHAIN:-nightly}"
    RTSAN_OPTIONS="suppressions=$(pwd)/.config/rtsan/async-suppressions.txt" \
    RUSTFLAGS="-Zsanitizer=realtime --cfg rtsan" \
    RUSTDOCFLAGS="-Zsanitizer=realtime --cfg rtsan" \
        cargo "+$toolchain" test -p kithara-integration-tests --test suite_light \
        --features kithara/no-block,kithara-test-utils/no-block --target "$target" -- --nocapture {{ FILTER }}

rtsan FILTER="offline_harness": (_rtsan "suite_light" FILTER)

rtsan-file FILTER="phase_continuity::file": (_rtsan "suite_stress" FILTER)

rtsan-hls FILTER="phase_continuity::hls": (_rtsan "suite_stress" FILTER)
