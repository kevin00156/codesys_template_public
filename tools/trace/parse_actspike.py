#!/usr/bin/env python3
"""Compare fSetVelocity vs fActVelocity to find following-error spikes, and
check whether fActPosition has matching position discontinuities.

Usage:
    parse_actspike.py <trace-or-xml-path>
"""

import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(__file__))
from trace_lib import load_trace

ACTIVE_STATES = (41, 60, 72)


def main(path):
    data = load_trace(path)
    sv = data['fSetVelocity']
    av = data['fActVelocity']
    sp = data['fSetPosition']
    ap = data['fActPosition']
    istep = data['iStep']

    print("=== fActVelocity stats in RUNNING (iStep=60) ===")
    av_run = [av[i] for i in range(len(av)) if istep[i] == 60]
    if av_run:
        print(f"  min={min(av_run):.2f}  max={max(av_run):.2f}"
              f"  mean={statistics.mean(av_run):.2f}"
              f"  median={statistics.median(av_run):.2f}")

    print("\n=== Top negative fActVelocity samples (iStep in 41/60/72) ===")
    neg = sorted(
        ((i, av[i]) for i in range(len(av)) if av[i] < -10 and istep[i] in ACTIVE_STATES),
        key=lambda x: x[1],
    )
    seen = []
    for idx, v in neg:
        if all(abs(idx - ni) > 100 for ni in seen):
            seen.append(idx)
            print(f"  idx={idx:6d}  fActVel={v:8.2f}  fSetVel={sv[idx]:7.2f}"
                  f"  fSetPos={sp[idx]:7.3f}  fActPos={ap[idx]:7.3f}"
                  f"  iStep={int(istep[idx])}")
            if len(seen) >= 10:
                break

    if seen:
        pick = sorted(seen)[len(seen) // 2]
        print(f"\n=== Window around fActVel spike at idx={pick} ===")
        for j in range(pick - 12, pick + 12):
            mark = " *" if j == pick else "  "
            print(f"{mark}idx={j:6d}  iStep={int(istep[j]):>3d}"
                  f"  fSetPos={sp[j]:7.3f}  fActPos={ap[j]:7.3f}"
                  f"  fSetVel={sv[j]:7.2f}  fActVel={av[j]:7.2f}")

    print("\n=== Top |ΔfActPosition| (360° modulo unwrap) in RUNNING ===")
    diffs = []
    for i in range(1, len(ap)):
        if istep[i] != 60:
            continue
        d = ap[i] - ap[i - 1]
        if abs(d) > 200:
            d = d - 360 if d > 0 else d + 360
        diffs.append((i, d))
    diffs.sort(key=lambda x: abs(x[1]), reverse=True)
    seen = []
    for idx, d in diffs[:50]:
        if all(abs(idx - ni) > 100 for ni in seen):
            seen.append(idx)
            print(f"  idx={idx:6d}  Δact={d:+.4f}"
                  f"  fActPos[{idx-1}]={ap[idx-1]:.3f}->[{idx}]={ap[idx]:.3f}"
                  f"  fActVel={av[idx]:7.2f}")
            if len(seen) >= 8:
                break


if __name__ == '__main__':
    if len(sys.argv) < 2:
        print(f"usage: {os.path.basename(__file__)} <trace-or-xml-path>",
              file=sys.stderr)
        sys.exit(2)
    main(sys.argv[1])
