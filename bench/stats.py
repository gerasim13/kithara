#!/usr/bin/env python3
"""Aggregate bench JSON lines and enforce benchmark equivalence."""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

METRICS = ["ttfa_ms", "wall_ms", "cpu_user_s", "cpu_sys_s", "max_rss_bytes"]
FRAME_TOLERANCE = 44_100
DEFAULT_DURATION_RANGE = (219.5, 220.5)


def iqr(values: list[float]) -> float:
    if len(values) < 2:
        return 0.0
    q1, _, q3 = statistics.quantiles(values, n=4, method="inclusive")
    return q3 - q1


def parse_lines(lines: list[str]) -> list[dict]:
    rows = []
    for line in lines:
        if line.strip():
            rows.append(json.loads(line))
    return rows


def check_equivalence(rows: list[dict], duration_range: tuple[float, float]) -> None:
    samplerates = {row["samplerate"] for row in rows}
    channels = {row["channels"] for row in rows}
    if len(samplerates) != 1:
        raise SystemExit(f"EQUIVALENCE FAIL: samplerates differ: {sorted(samplerates)}")
    if len(channels) != 1:
        raise SystemExit(f"EQUIVALENCE FAIL: channels differ: {sorted(channels)}")

    frames = [row["pcm_frames"] for row in rows]
    if frames and max(frames) - min(frames) > FRAME_TOLERANCE:
        raise SystemExit(
            "EQUIVALENCE FAIL: pcm_frames exceed 1s tolerance: "
            f"min={min(frames)} max={max(frames)} tolerance={FRAME_TOLERANCE}"
        )

    min_duration, max_duration = duration_range
    for row in rows:
        duration = row["pcm_frames"] / row["samplerate"]
        if not (min_duration <= duration <= max_duration):
            engine = row["engine"]
            decoder = row["decoder"]
            raise SystemExit(
                "EQUIVALENCE FAIL: "
                f"{engine}/{decoder} duration {duration:.3f}s outside "
                f"[{min_duration:.3f}, {max_duration:.3f}]"
            )


def aggregate(lines: list[str], duration_range: tuple[float, float] = DEFAULT_DURATION_RANGE):
    rows = parse_lines(lines)
    if not rows:
        raise SystemExit("no results to aggregate")
    check_equivalence(rows, duration_range)

    groups: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for row in rows:
        groups[(row["engine"], row["decoder"])].append(row)

    out = {}
    for key, group_rows in groups.items():
        out[key] = {}
        for metric in METRICS:
            values = sorted(float(row[metric]) for row in group_rows)
            out[key][metric] = (statistics.median(values), iqr(values))
    return out


def self_test() -> int:
    base = {
        "samplerate": 44100,
        "channels": 2,
        "wall_ms": 1,
        "cpu_user_s": 1,
        "cpu_sys_s": 1,
        "max_rss_bytes": 1,
    }
    lines = [
        json.dumps(
            {
                **base,
                "engine": "kithara",
                "decoder": "symphonia",
                "pcm_frames": 9_712_640,
                "ttfa_ms": ttfa,
            }
        )
        for ttfa in (10, 20, 30)
    ]
    agg = aggregate(lines)
    assert agg[("kithara", "symphonia")]["ttfa_ms"][0] == 20, agg

    tolerance_pass = lines + [
        json.dumps(
            {
                **base,
                "engine": "superpowered",
                "decoder": "superpowered",
                "pcm_frames": 9_702_400,
                "ttfa_ms": 198,
            }
        )
    ]
    aggregate(tolerance_pass)

    over_tolerance = lines + [
        json.dumps(
            {
                **base,
                "engine": "superpowered",
                "decoder": "superpowered",
                "pcm_frames": 9_712_640 - FRAME_TOLERANCE - 1,
                "ttfa_ms": 198,
            }
        )
    ]
    try:
        aggregate(over_tolerance)
    except SystemExit:
        pass
    else:
        raise AssertionError("frame tolerance gate did not fire")

    bad_duration = [
        json.dumps(
            {
                **base,
                "engine": "kithara",
                "decoder": "symphonia",
                "pcm_frames": 1_000,
                "ttfa_ms": 1,
            }
        )
    ]
    try:
        aggregate(bad_duration)
    except SystemExit:
        pass
    else:
        raise AssertionError("duration gate did not fire")

    print("self-test OK")
    return 0


def print_table(agg) -> None:
    for (engine, decoder), metrics in sorted(agg.items()):
        cells = [
            f"{metric}={median:.2f}+/-{spread:.2f}"
            for metric, (median, spread) in metrics.items()
        ]
        print(f"{engine:12s} {decoder:12s} {'  '.join(cells)}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("results", nargs="?")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--duration-range", nargs=2, type=float, metavar=("MIN", "MAX"))
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()
    if not args.results:
        parser.error("results path is required unless --self-test is used")

    duration_range = tuple(args.duration_range or DEFAULT_DURATION_RANGE)
    lines = Path(args.results).read_text().splitlines()
    print_table(aggregate(lines, duration_range))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
