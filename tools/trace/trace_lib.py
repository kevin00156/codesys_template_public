#!/usr/bin/env python3
"""Shared loader for trace recordings.

Two formats are accepted, auto-detected by content:

* CODESYS trace exports — `.trace` files are UTF-16 LE XML with a BOM; the
  IDE also exports a UTF-8 XML copy when you "Save Trace as XML".
* `plc-trace/1` columnar JSON — the Rust motion-daemon recordings, produced
  by the HMI's "Export recording" button or `GET /api/trace/export`.

Both load into the same shape, so every analysis script here works on either.
"""

import json
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
    """Return {alias: [float, ...]} from a trace recording (either format).

    `alias` is the last dotted segment of the recorded name (e.g.
    `Application.PLC_PRG.fSetVelocity` -> `fSetVelocity`, `axis0.actPos` ->
    `actPos`). Aliases that collide keep their first occurrence; plc-trace
    files additionally keep every full field name.
    """
    with open(path, 'rb') as f:
        head = f.read(64)
    if head.lstrip(b'\xff\xfe\xef\xbb\xbf \t\r\n').startswith(b'{'):
        return load_plc_trace(path)

    content = _read_xml(path)
    data = {}
    for m in _VAR_PATTERN.finditer(content):
        alias = m.group(1).split('.')[-1]
        if alias in data:
            continue
        data[alias] = [float(v) for v in m.group(2).split(',') if v.strip()]
    return data


def load_plc_trace(path):
    """Return {name: [float, ...]} from a `plc-trace/1` JSON recording.

    Full field names (`axis0.actPos`, `periodNs`, ...) are always present;
    the last dotted segment is added as an alias when unambiguous (first
    occurrence wins, matching load_trace's collision rule — so `actPos`
    means axis 0).
    """
    with open(path, 'r', encoding='utf-8') as f:
        doc = json.load(f)
    if doc.get('format') != 'plc-trace/1':
        raise ValueError(f'{path}: not a plc-trace/1 file (format={doc.get("format")!r})')
    fields, columns = doc['fields'], doc['columns']
    if len(fields) != len(columns):
        raise ValueError(f'{path}: fields/columns length mismatch')
    data = {}
    for name, col in zip(fields, columns):
        data[name] = [float(v) for v in col]
    for name in list(data):
        alias = name.split('.')[-1]
        if alias not in data:
            data[alias] = data[name]
    return data
