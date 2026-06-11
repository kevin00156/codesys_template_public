#!/usr/bin/env python3
"""Velocity profile across one RUNNING (iStep=60) cycle: fSetVelocity stats,
histogram, and scan-to-scan Δv drops worth investigating.

Usage:
    parse_velprofile.py <trace-or-xml-path>
"""

import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(__file__))
from trace_lib import load_trace


def main(path):
    data = load_trace(path)
    sv = data['fSetVelocity']
    av = data['fActVelocity']
    sp = data['fSetPosition']
    istep = data['iStep']

    run_start = None
    run_end = None
    for i, v in enumerate(istep):
        if v == 60 and run_start is None:
            run_start = i
        if v != 60 and run_start is not None and run_end is None:
            run_end = i
            break
    if run_end is None:
        run_end = len(istep)
    if run_start is None:
        print("no RUNNING (iStep=60) samples in trace", file=sys.stderr)
        return

    print(f"RUNNING range: idx {run_start} .. {run_end - 1}"
          f"  ({run_end - run_start} samples)")
    sv_run = sv[run_start:run_end]
    print("\nfSetVelocity stats in RUNNING:")
    print(f"  min={min(sv_run):.2f}  max={max(sv_run):.2f}"
          f"  mean={statistics.mean(sv_run):.2f}"
          f"  median={statistics.median(sv_run):.2f}")

    buckets = [(-1000, -100), (-100, -50), (-50, -10), (-10, 0),
               (0, 10), (10, 50), (50, 100), (100, 150), (150, 200), (200, 1000)]
    print("\nfSetVel histogram (RUNNING):")
    for lo, hi in buckets:
        n = sum(1 for v in sv_run if lo <= v < hi)
        print(f"  [{lo:5d}, {hi:5d}): {n:6d}")

    print("\n=== Sharp Δv drops in RUNNING (Δv < -20°/s between consecutive scans) ===")
    drops = []
    for i in range(run_start + 1, run_end):
        dv = sv[i] - sv[i - 1]
        if dv < -20:
            drops.append((i, dv, sv[i - 1], sv[i]))
    print(f"Total drops with |Δv|>20: {len(drops)}")

    drops.sort(key=lambda x: x[1])
    seen = []
    filtered = []
    for idx, dv, prev, now in drops:
        if all(abs(idx - ni) > 50 for ni in seen):
            seen.append(idx)
            filtered.append((idx, dv, prev, now))
            if len(filtered) >= 15:
                break
    for idx, dv, prev, now in sorted(filtered):
        print(f"  idx={idx:6d}  Δv={dv:8.2f}  prev_vel={prev:7.2f} -> now_vel={now:7.2f}"
              f"  prev_pos={sp[idx-1]:7.3f}  pos={sp[idx]:7.3f}")

    if filtered:
        pick = filtered[len(filtered) // 2][0]
        print(f"\n=== Window around drop at idx={pick} ===")
        for j in range(pick - 10, pick + 10):
            mark = " *" if j == pick else "  "
            print(f"{mark}idx={j:6d}  fSetPos={sp[j]:7.3f}"
                  f"  fSetVel={sv[j]:8.2f}  fActVel={av[j]:8.2f}"
                  f"  iStep={int(istep[j])}")


if __name__ == '__main__':
    if len(sys.argv) < 2:
        print(f"usage: {os.path.basename(__file__)} <trace-or-xml-path>",
              file=sys.stderr)
        sys.exit(2)
    main(sys.argv[1])
