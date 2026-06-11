#!/usr/bin/env python3
"""Shared loader for CODESYS trace exports.

`.trace` files are UTF-16 LE XML with a BOM; the IDE also exports a UTF-8 XML
copy when you "Save Trace as XML". Both are accepted.
"""

import re

_VAR_PATTERN = re.compile(
    r'<TraceVariable VarName="([^"]+)"[^>]*>\s*<Values>([^<]+)</Values>',
    re.MULTILINE,
)


def _read_xml(path):
    with open(path, 'rb') as f:
        raw = f.read()
    if raw[:2] == b'\xff\xfe':
        return raw[2:].decode('utf-16-le')
    return raw.decode('utf-8')


def load_trace(path):
    """Return {alias: [float, ...]} from a CODESYS trace export.

    `alias` is the last dotted segment of the recorded `VarName` (e.g.
    `Application.PLC_PRG.fSetVelocity` -> `fSetVelocity`). Aliases that collide
    keep their first occurrence.
    """
    content = _read_xml(path)
    data = {}
    for m in _VAR_PATTERN.finditer(content):
        alias = m.group(1).split('.')[-1]
        if alias in data:
            continue
        data[alias] = [float(v) for v in m.group(2).split(',') if v.strip()]
    return data
