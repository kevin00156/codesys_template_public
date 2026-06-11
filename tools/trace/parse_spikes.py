#!/usr/bin/env python3
"""Find negative fSetVelocity spikes inside the CK_Crosscutter active states
(iStep 41 / 60 / 72), then report inter-spike timing in RUNNING (iStep=60).

Usage:
    parse_spikes.py <trace-or-xml-path>
"""

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from trace_lib import load_trace

ACTIVE_STATES = (41, 60, 72)
SCAN_PERIOD_MS = 0.25  # 250 µs CODESYS scan


def main(path):
    data = load_trace(path)
    sv = data['fSetVelocity']
    sp = data['fSetPosition']
    istep = data['iStep']

    print("=== Negative fSetVelocity spikes in active states (iStep 41/60/72) ===")
    spikes = []
    for i in range(2, len(sv) - 2):
        if sv[i] < -30 and sv[i - 1] > -5 and sv[i + 1] > -5 and istep[i] in ACTIVE_STATES:
            spikes.append((i, sv[i]))

    # one spike per region
    seen = []
    filtered = []
    for idx, v in sorted(spikes, key=lambda x: abs(x[1]), reverse=True):
        if all(abs(idx - ni) > 50 for ni in seen):
            seen.append(idx)
            filtered.append((idx, v))
            if len(filtered) >= 20:
                break

    for idx, v in sorted(filtered):
        print(f"  idx={idx:6d}  fSetVel={v:8.2f}  fSetPos={sp[idx]:7.3f}"
              f"  iStep={int(istep[idx])}"
              f"  prev_pos={sp[idx-1]:7.3f}  prev_vel={sv[idx-1]:6.2f}")

    print("\n=== Time gaps between negative spikes in RUNNING (iStep=60) ===")
    run_spikes = sorted(idx for idx, _ in spikes if istep[idx] == 60)
    dedup = []
    for idx in run_spikes:
        if not dedup or idx - dedup[-1] > 50:
            dedup.append(idx)
    gaps_ms = [(dedup[i] - dedup[i - 1]) * SCAN_PERIOD_MS for i in range(1, len(dedup))]
    print(f"  spike count in RUNNING: {len(dedup)}")
    if gaps_ms:
        gaps_sorted = sorted(gaps_ms)
        print(f"  gap (ms): min={gaps_sorted[0]:.1f}  max={gaps_sorted[-1]:.1f}"
              f"  median={gaps_sorted[len(gaps_sorted)//2]:.1f}")

    # Detailed window around the first spike past idx 30000 (a typical RUNNING one)
    pick = next((idx for idx in dedup if idx > 30000), None)
    if pick is not None and 'lrDbg_CamXAtExec' in data and 'lrDbg_CamYAtExec' in data:
        print(f"\n=== Detail window around RUNNING spike at idx={pick} ===")
        for j in range(pick - 8, pick + 8):
            mark = " *" if j == pick else "  "
            print(f"{mark}idx={j:6d}  iStep={int(istep[j])}"
                  f"  fSetPos={sp[j]:7.3f}  fSetVel={sv[j]:8.2f}"
                  f"  camX={data['lrDbg_CamXAtExec'][j]:7.3f}"
                  f"  camY={data['lrDbg_CamYAtExec'][j]:7.3f}")


if __name__ == '__main__':
    if len(sys.argv) < 2:
        print(f"usage: {os.path.basename(__file__)} <trace-or-xml-path>",
              file=sys.stderr)
        sys.exit(2)
    main(sys.argv[1])
