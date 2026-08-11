set shell := ["bash", "-euo", "pipefail", "-c"]
set working-directory := "../.."

# Execute one typed CI lane. GitLab jobs should only route through this recipe.
[positional-arguments]
run *ARGS:
    just _xtask ci run "$@"

# Provision or maintain the dedicated CI host.
[positional-arguments]
host *ARGS:
    just _xtask ci host "$@"

# Build a pinned CI container image for this machine's architecture.
[positional-arguments]
image *ARGS:
    just _xtask ci image "$@"

# Provision or maintain a Linux CI machine and the runners it serves.
[positional-arguments]
linux *ARGS:
    just _xtask ci linux "$@"

# Reconcile the private GitLab repository with the public GitHub repository.
[positional-arguments]
bridge *ARGS:
    just _xtask ci bridge "$@"

# Run all linters scoped to a crate, path, or workspace.
[positional-arguments]
audit *ARGS:
    just _xtask audit "$@"

# Dual-run gate: flash ON with no-block, then flash OFF.
[positional-arguments]
gate *ARGS:
    #!/usr/bin/env bash
    set -eo pipefail
    echo "=== gate: flash ON + no-block ON ==="
    just test run --flash=on --no-block=on "$@"
    echo "=== gate: flash OFF ==="
    CARGO_TARGET_DIR=target-flash-off just test run --flash=off "$@"

# Comprehensive workspace health report.
health:
    just _xtask health

# Full CI cycle with independent stage logs.
full OUTPUT:
    #!/usr/bin/env bash
    set -uo pipefail
    RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
    mkdir -p "{{ OUTPUT }}"
    echo "$RUN_ID" > "{{ OUTPUT }}/run-id"
    echo "=== ci full $RUN_ID starting at $(date -u -Iseconds) ==="

    run_stage() {
        local name=$1; shift
        echo ">>> stage $name started at $(date -u -Iseconds)"
        "$@" > "{{ OUTPUT }}/stage-$name.log" 2>&1
        local rc=$?
        echo "$rc" > "{{ OUTPUT }}/stage-$name.exit"
        echo ">>> stage $name finished with exit=$rc at $(date -u -Iseconds)"
    }

    run_stage lint    just lint fast
    run_stage test    just test
    # A Linux container has no output device; the hardware e2e lane runs as `apple:e2e`.
    run_stage mutants just test mutants run-all --output "{{ OUTPUT }}/mutants"

    echo "=== ci full $RUN_ID done at $(date -u -Iseconds) ==="
