#!/usr/bin/env python3
"""Dump every recorded variable at the given trace indices. Useful for
zooming into specific moments identified by parse_trace.py / parse_spikes.py.

Usage:
    dump_idx.py <trace-or-xml-path> <idx> [<idx> ...]
"""

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from trace_lib import load_trace


def main(path, indices):
    data = load_trace(path)
    length = len(next(iter(data.values())))

    for idx in indices:
        if idx < 0 or idx >= length:
            print(f"\n=== idx={idx} OUT OF RANGE (trace has {length} samples) ===")
            continue
        print(f"\n=== idx={idx} ===")
        for name, values in data.items():
            print(f"  {name:30s} = {values[idx]}")


if __name__ == '__main__':
    if len(sys.argv) < 3:
        print(f"usage: {os.path.basename(__file__)} <trace-or-xml-path> <idx> [<idx> ...]",
              file=sys.stderr)
        sys.exit(2)
    try:
        indices = [int(a) for a in sys.argv[2:]]
    except ValueError as e:
        print(f"error: indices must be integers ({e})", file=sys.stderr)
        sys.exit(2)
    main(sys.argv[1], indices)
