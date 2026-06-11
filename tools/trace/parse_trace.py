#!/usr/bin/env python3
"""Overview of a CODESYS trace: iStep transitions, R_TRIG engage events, top
|fSetVelocity| samples, top |ΔfSetPosition| jumps (with 360° modulo unwrap),
and a small window around each engage event.

Usage:
    parse_trace.py <trace-or-xml-path>
"""

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from trace_lib import load_trace


def _dedup_take(indexed, key_fn, max_count, min_gap):
    """Yield at most `max_count` items, sorted by `key_fn(item)` desc, skipping
    items whose index is within `min_gap` of an already-yielded one."""
    indexed = sorted(indexed, key=key_fn, reverse=True)
    chosen = []
    for item in indexed:
        idx = item[0]
        if all(abs(idx - c[0]) >= min_gap for c in chosen):
            chosen.append(item)
            if len(chosen) >= max_count:
                break
    return chosen


def main(path):
    data = load_trace(path)

    print("=== variables ===")
    for alias, values in data.items():
        print(f"{alias:30s} len={len(values):6d}  first={values[0]:.3f}  last={values[-1]:.3f}")

    istep = data['iStep']
    sp = data['fSetPosition']
    sv = data['fSetVelocity']
    av = data.get('fActVelocity', [0.0] * len(sv))
    eng = data['udiDbg_EngageScan']

    print("\n=== iStep transitions ===")
    prev = None
    for i, v in enumerate(istep):
        if v != prev:
            if prev is not None:
                print(f"  idx={i:6d}  iStep: {int(prev):>5d} -> {int(v):>5d}")
            prev = v

    print("\n=== udiDbg_EngageScan increments (R_TRIG events) ===")
    prev = None
    for i, v in enumerate(eng):
        if v != prev and prev is not None and v > prev:
            print(f"  idx={i:6d}  EngageScan: {int(prev)} -> {int(v)}")
            print(f"    iStep={int(istep[i])}"
                  f"  MasterAtEngage={data['lrDbg_MasterAtEngage'][i]:.3f}"
                  f"  SlaveAtEngage={data['lrDbg_SlaveAtEngage'][i]:.3f}"
                  f"  Buf={int(data['iDbg_BufAtEngage'][i])}"
                  f"  CamYInit={data['lrDbg_CamYInitAtCamIn'][i]:.3f}"
                  f"  SlaveOffset={data['lrDbg_SlaveOffset'][i]:.3f}")
        prev = v

    print("\n=== Top |fSetVelocity| samples ===")
    top_v = _dedup_take(list(enumerate(sv)), key_fn=lambda x: abs(x[1]),
                        max_count=8, min_gap=5)
    for idx, v in sorted(top_v):
        print(f"  idx={idx:6d}  fSetVelocity={v:.1f}  fActVelocity={av[idx]:.1f}"
              f"  fSetPos={sp[idx]:.3f}  iStep={int(istep[idx])}"
              f"  EngageScan={int(eng[idx])}")

    print("\n=== Top |ΔfSetPosition| jumps (360° modulo unwrap) ===")
    diffs = []
    for i in range(1, len(sp)):
        d = sp[i] - sp[i - 1]
        # cam table is modulo 360°; treat large jumps as wraparound
        if abs(d) > 200:
            d = d - 360 if d > 0 else d + 360
        diffs.append((i, d))
    top_d = _dedup_take(diffs, key_fn=lambda x: abs(x[1]),
                        max_count=8, min_gap=5)
    for idx, d in sorted(top_d):
        print(f"  idx={idx:6d}  Δ={d:+.3f}  [{idx-1}]={sp[idx-1]:.3f}"
              f" -> [{idx}]={sp[idx]:.3f}"
              f"  iStep={int(istep[idx])}  EngageScan={int(eng[idx])}")

    print("\n=== Detailed window around each engage event ===")
    prev = None
    for i, v in enumerate(eng):
        if v != prev and prev is not None and v > prev:
            print(f"\n--- Engage event #{int(v)} at idx={i} ---")
            for j in range(max(0, i - 3), min(len(eng), i + 8)):
                mark = " *" if j == i else "  "
                print(f"{mark} idx={j:6d}  iStep={int(istep[j]):>5d}"
                      f"  Buf={int(data['iDbg_BufAtEngage'][j])}"
                      f"  fSetPos={sp[j]:>8.3f}  fSetVel={sv[j]:>9.1f}"
                      f"  MstAtEng={data['lrDbg_MasterAtEngage'][j]:>8.3f}"
                      f"  SlvAtEng={data['lrDbg_SlaveAtEngage'][j]:>8.3f}"
                      f"  CamYInit={data['lrDbg_CamYInitAtCamIn'][j]:>8.3f}"
                      f"  SlvOff={data['lrDbg_SlaveOffset'][j]:>8.3f}"
                      f"  MstOff={data['lrDbg_MasterOffset'][j]:>9.3f}")
        prev = v


if __name__ == '__main__':
    if len(sys.argv) < 2:
        print(f"usage: {os.path.basename(__file__)} <trace-or-xml-path>",
              file=sys.stderr)
        sys.exit(2)
    main(sys.argv[1])
