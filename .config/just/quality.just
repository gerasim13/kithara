set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

[positional-arguments]
report *ARGS:
    just _xtask quality report "$@"

unimock-check:
    just _xtask quality unimock-check

rstest-audit:
    just _xtask quality rstest-audit

trait-mock-audit:
    just _xtask quality trait-mock-audit

trait-mock-exceptions:
    just _xtask quality trait-mock-exceptions

# Heavyweight analyzers kept outside lint-fast, audit, and pre-commit.
[positional-arguments]
lab *ARGS:
    just _xtask quality lab "$@"

# Run cargo-crap against the LCOV report.
[positional-arguments]
coverage-risk *ARGS:
    just test coverage
    just quality lab run coverage --lcov "${COVERAGE_OUTPUT_DIR:-./coverage}/lcov.info" "$@"

# CI-tuned quality report; thresholds come from QUALITY_* variables.
report-ci:
    just _xtask quality report \
        --min-unimock-traits "${QUALITY_MIN_UNIMOCK_TRAITS:-0}" \
        --min-rstest-cases "${QUALITY_MIN_RSTEST_CASES:-0}" \
        --min-perf-test-files "${QUALITY_MIN_PERF_TEST_FILES:-0}" \
        --min-bench-targets "${QUALITY_MIN_BENCH_TARGETS:-0}" \
        --max-local-http-servers "${QUALITY_MAX_LOCAL_HTTP_SERVERS:-999}"
