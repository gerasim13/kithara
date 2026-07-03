#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

usage() {
  cat >&2 <<'EOF'
usage: bench/run.sh [N] [--paced] [--scenario hls|progressive|local] [--kit-url <url> --sp-url <url>] [--duration-range <min> <max>]
EOF
}

N=5
PACED_ARGS=()
KIT_URL_OVERRIDE=""
SP_URL_OVERRIDE=""
DURATION_ARGS=()
SCENARIO="hls"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --paced)
      PACED_ARGS=(--paced)
      shift
      ;;
    --kit-url)
      KIT_URL_OVERRIDE="${2:-}"
      [[ -n "$KIT_URL_OVERRIDE" ]] || { usage; exit 2; }
      shift 2
      ;;
    --sp-url)
      SP_URL_OVERRIDE="${2:-}"
      [[ -n "$SP_URL_OVERRIDE" ]] || { usage; exit 2; }
      shift 2
      ;;
    --scenario)
      SCENARIO="${2:-}"
      case "$SCENARIO" in
        hls|progressive|local) ;;
        *) usage; exit 2 ;;
      esac
      shift 2
      ;;
    --duration-range)
      [[ $# -ge 3 ]] || { usage; exit 2; }
      DURATION_ARGS=(--duration-range "$2" "$3")
      shift 3
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      N="$1"
      shift
      ;;
  esac
done

if [[ -z "$KIT_URL_OVERRIDE" && -n "$SP_URL_OVERRIDE" ]] || [[ -n "$KIT_URL_OVERRIDE" && -z "$SP_URL_OVERRIDE" ]]; then
  echo "error: --kit-url and --sp-url must be provided together" >&2
  exit 2
fi

if [[ "$SCENARIO" == "local" && ( -n "$KIT_URL_OVERRIDE" || -n "$SP_URL_OVERRIDE" ) ]]; then
  echo "error: --kit-url/--sp-url are not valid with --scenario local" >&2
  exit 2
fi

: "${SUPERPOWERED_SDK_DIR:?set SUPERPOWERED_SDK_DIR to the Superpowered SDK checkout}"

RATE="${BENCH_RATE:-44100}"
if [[ -z "${BENCH_TMPDIR:-}" ]]; then
  mkdir -p /Volumes/Render/dev/tmp
  BENCH_TMPDIR="$(mktemp -d /Volumes/Render/dev/tmp/kithara-bench.XXXXXX)"
fi
mkdir -p "$BENCH_TMPDIR"
export TMPDIR="$BENCH_TMPDIR"
RESULTS="$BENCH_TMPDIR/results.jsonl"
ERRORS="$BENCH_TMPDIR/errors.log"
: > "$RESULTS"
: > "$ERRORS"

SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

echo "== build phase (unmeasured) =="
KIT_TARGET_SYM="${KIT_TARGET_SYM:-/Volumes/Render/dev/cargo-targets/kithara-bench-kit}"
KIT_TARGET_APL="${KIT_TARGET_APL:-/Volumes/Render/dev/cargo-targets/kithara-bench-kit-apple}"
( cd bench/kit-bench && cargo build --release --target-dir "$KIT_TARGET_SYM" )
( cd bench/kit-bench && cargo build --release --no-default-features --features apple --target-dir "$KIT_TARGET_APL" )
( cd bench/sp-bench && make )

echo "== preflight =="
{
  echo "date: $(date -u +%FT%TZ)"
  sw_vers
  sysctl -n machdep.cpu.brand_string
  rustc --version
  clang++ --version | head -1
  echo "git: $(git rev-parse --short HEAD)"
  echo "BENCH_RATE: $RATE"
  echo "BENCH_TMPDIR: $BENCH_TMPDIR"
  echo "scenario: $SCENARIO"
  echo "profiles: kit=release kit-apple=release sp=-O2"
  [[ -n "${RUSTFLAGS:-}" ]] && echo "WARN: RUSTFLAGS=$RUSTFLAGS"
  [[ -n "${CXXFLAGS:-}" ]] && echo "WARN: CXXFLAGS=$CXXFLAGS"
  [[ -n "${MallocStackLogging:-}${MallocScribble:-}" ]] && echo "WARN: Malloc debug env set"
  uptime
} | tee "$BENCH_TMPDIR/preflight.txt"

KIT_MODE_ARGS=()
SP_MODE_ARGS=()
if [[ "$SCENARIO" != "hls" ]]; then
  KIT_MODE_ARGS=(--mode "$SCENARIO")
  SP_MODE_ARGS=(--mode "$SCENARIO")
fi

if [[ "$SCENARIO" == "local" ]]; then
  KIT_URL="$PWD/bench/fixtures/shq.aac"
  SP_URL="$PWD/bench/fixtures/shq.aac"
elif [[ -n "$KIT_URL_OVERRIDE" ]]; then
  KIT_URL="$KIT_URL_OVERRIDE"
  SP_URL="$SP_URL_OVERRIDE"
else
  echo "== server =="
  python3 -u -m http.server 0 --bind 127.0.0.1 > "$BENCH_TMPDIR/server.log" 2>&1 &
  SERVER_PID=$!
  PORT=""
  for _ in $(seq 1 100); do
    PORT="$(sed -n 's/^Serving HTTP on 127\.0\.0\.1 port \([0-9][0-9]*\).*/\1/p' "$BENCH_TMPDIR/server.log" | head -1)"
    if [[ -n "$PORT" ]]; then
      break
    fi
    sleep 0.1
  done
  [[ -n "$PORT" ]] || { echo "server did not start"; cat "$BENCH_TMPDIR/server.log" >&2; exit 1; }
  BASE="http://127.0.0.1:$PORT"
  case "$SCENARIO" in
    hls)
      KIT_URL="$BASE/bench/fixtures/master-shq.m3u8"
      SP_URL="$BASE/bench/fixtures-ts/shq/index.m3u8"
      ;;
    progressive)
      KIT_URL="$BASE/bench/fixtures/shq.aac"
      SP_URL="$BASE/bench/fixtures/shq.aac"
      ;;
  esac
  for _ in $(seq 1 100); do
    if curl -fsS "$KIT_URL" >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
  curl -fsS "$KIT_URL" >/dev/null
fi

echo "kit url: $KIT_URL"
echo "sp url: $SP_URL"
echo "scenario: $SCENARIO"
echo "rate: $RATE"

KIT_SYM="$KIT_TARGET_SYM/release/kit-bench"
KIT_APL="$KIT_TARGET_APL/release/kit-bench"
SP=bench/sp-bench/sp-bench

run_one() {
  local tag="$1"
  shift
  local out
  if ! out=$("$@" 2>>"$ERRORS"); then
    echo "RUN FAILED: $tag (see $ERRORS)" >&2
    exit 1
  fi
  local non_empty
  non_empty="$(printf '%s\n' "$out" | sed '/^[[:space:]]*$/d' | wc -l | tr -d ' ')"
  if [[ "$non_empty" != "1" ]]; then
    echo "RUN FAILED: $tag produced $non_empty stdout lines" >&2
    printf '%s\n' "$out" >&2
    exit 1
  fi
  printf '%s\n' "$out" >> "$RESULTS"
  echo "  $tag: $out"
}

echo "== warm-up (unmeasured) =="
WARM_TMP="$(mktemp -d "$BENCH_TMPDIR/warm.XXXXXX")"
mkdir -p "$WARM_TMP/sp"
"$SP" "$SP_URL" "$RATE" "${PACED_ARGS[@]}" "${SP_MODE_ARGS[@]}" --tmp "$WARM_TMP/sp" >/dev/null 2>>"$ERRORS"
"$KIT_SYM" "$KIT_URL" "${PACED_ARGS[@]}" "${KIT_MODE_ARGS[@]}" >/dev/null 2>>"$ERRORS"
"$KIT_APL" "$KIT_URL" "${PACED_ARGS[@]}" "${KIT_MODE_ARGS[@]}" >/dev/null 2>>"$ERRORS"
rm -rf "$WARM_TMP"

echo "== measured: $N reps, rotated order =="
for i in $(seq 1 "$N"); do
  case $((i % 3)) in
    1) ORDER=(sp sym apl) ;;
    2) ORDER=(sym apl sp) ;;
    0) ORDER=(apl sp sym) ;;
  esac
  for side in "${ORDER[@]}"; do
    case "$side" in
      sp)
        SP_TMP="$(mktemp -d "$BENCH_TMPDIR/sp-$i.XXXXXX")"
        run_one "sp[$i]" "$SP" "$SP_URL" "$RATE" "${PACED_ARGS[@]}" "${SP_MODE_ARGS[@]}" --tmp "$SP_TMP"
        rm -rf "$SP_TMP"
        ;;
      sym)
        run_one "sym[$i]" "$KIT_SYM" "$KIT_URL" "${PACED_ARGS[@]}" "${KIT_MODE_ARGS[@]}"
        ;;
      apl)
        run_one "apl[$i]" "$KIT_APL" "$KIT_URL" "${PACED_ARGS[@]}" "${KIT_MODE_ARGS[@]}"
        ;;
    esac
  done
done

echo "== results =="
cat "$BENCH_TMPDIR/preflight.txt"
python3 bench/stats.py "${DURATION_ARGS[@]}" "$RESULTS"
cp "$RESULTS" bench/last-results.jsonl
echo "raw: bench/last-results.jsonl"
echo "tmp: $BENCH_TMPDIR"
