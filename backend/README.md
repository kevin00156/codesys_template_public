# plc_bridge

PLC writes typed data to `/dev/shm`. Go reads it, holds a snapshot, and
re-publishes it as Modbus TCP (and later: HTTP/WebSocket for Svelte).

The PLC has no Modbus code. There is no register table inside CODESYS.
The register layout lives in Go and is one file:
[internal/modbus/addresses.go](internal/modbus/addresses.go).

## Why

Putting Modbus inside the PLC is what we did before
(`Device/Application/Programs/ModbusProgram` — legacy, frozen).
Maintaining a flat 16-bit register array next to the control logic
forced manual offset arithmetic, alignment hazards, and two sources
of truth whenever a new variable was added.

This bridge makes the PLC publish *typed* state — one struct, with
named fields. Go is the only place that knows about Modbus addressing,
JSON, WebSocket, etc. Adding a new external interface never touches
the PLC.

## Topology

```
  CODESYS RT task                     external SCADA / HMI
        |                                       |
        | writes PlcData (seqlock)              | Modbus TCP (:5020)
        v                                       v
  /dev/shm/plc_data            +----------------------------+
  /dev/shm/plc_cmd  <--------- |   plc_bridge (Go)          |
                               |   - shm reader (10 ms)     |
                               |   - state.Snapshot         |
                               |   - modbus tcp server      |
                               +----------------------------+
```

PLC and Go agree on **two** things: the byte layout of `PlcData` and
`PlcCommand`, and their `(magic, version)` headers. Everything else is
internal to one side.

## Layout contract

| File | Side | Role |
|---|---|---|
| [../codesys_export/Device/Application/](../codesys_export/Device/Application/) | PLC | DUTs + GVL + publisher POU, auto-exported from the CODESYS IDE. One file per IDE object. |
| [internal/shm/layout.go](internal/shm/layout.go) | Go | mirror DUTs |

Both files declare `Magic`, `Version`. **Bump `Version` on every layout
change**, in *both* files. The Go reader refuses to mount a segment
whose version it does not recognise, instead of silently reading
garbage.

## Adding a new published variable

1. Add the field to a payload struct (`MachineState`, `MotionState`,
   or a new domain struct) in:
   - the matching DUT under [../codesys_export/Device/Application/DUT/ShmBridge/](../codesys_export/Device/Application/DUT/ShmBridge/)
   - [internal/shm/layout.go](internal/shm/layout.go)
2. Bump `PLC_DATA_VERSION` / `PlcDataVersion` in both.
3. If you want it on Modbus, add an address constant + an `EncodeData`
   line in [internal/modbus/addresses.go](internal/modbus/addresses.go).

## Adding a new write (Modbus master → PLC)

Same pattern, but on `PlcCommand` and `commandFields` in
[internal/modbus/addresses.go](internal/modbus/addresses.go).

## Deploy

```bash
make deploy   # build web + go, rsync, sudo install, systemctl restart
make rollback # revert to previous binary
make logs     # follow journald
```

## Remote one-time setup

### shm permissions (`/etc/systemd/system/plc_bridge.service.d/shm-perms.conf`)
CODESYS creates shm files as `rwxr-x---`. This drop-in chowns them to `664`
before plc_bridge opens them, so `plc_bridge` user (in root group) can write `plc_cmd`.

```ini
[Service]
ExecStartPre=-+/bin/chmod 664 /dev/shm/plc_data /dev/shm/plc_cmd
```

After creating: `sudo systemctl daemon-reload`

## shm layout versions

| Segment    | Size  | Version | Direction      |
|------------|-------|---------|----------------|
| `plc_data` | 88 B  | v2      | PLC → Go → HMI |
| `plc_cmd`  | 48 B  | v1      | HMI → Go → PLC |

Version mismatch → plc_bridge logs error and exits (Restart=on-failure retries until PLC is updated).

## Running

```bash
# 1. PLC must already be publishing /dev/shm/plc_data and /dev/shm/plc_cmd.
# 2. Pin to a non-isolated CPU (PLC owns CPU 2-3).
taskset -c 0 go run ./backend/cmd/plc_bridge
```

Flags: `--modbus :5020` `--modbus-allow ""` `--http :8080` `--poll 10ms` `--push 100ms`

## Security posture

This bridge sits between an industrial machine and the network, so the
write paths are gated:

- **HTTP + WebSocket auth.** Role passwords (vendor ⊇ tuner ⊇ operator) come
  from `PLC_BRIDGE_*_HASH` env vars (bcrypt; mint with `plc_bridge -gen-hash`).
  With none set, auth is disabled — every route and the command plane are open
  (the dev posture). The login wall covers **both** HTTP routes *and the
  WebSocket command plane*: an unauthenticated socket may watch telemetry but
  cannot command the machine. Per-command minimums live in
  `wsserver.commandRole` (machine/production → operator, axis → tuner). Failed
  logins are throttled per-IP; every command is audit-logged (peer, role, type,
  outcome).
- **TLS.** Drop `cert.pem`/`key.pem` in the working dir to auto-enable HTTPS;
  the session cookie is `Secure` only then. Over plain HTTP the bridge logs a
  warning — the password and cookie travel in clear, so install certs for
  production.
- **Modbus has no authentication** (protocol limitation). The documented
  topology is SCADA on a trusted LAN. In production restrict the surface with
  `--modbus-allow` (comma-separated IPs/CIDRs, e.g.
  `--modbus-allow 192.168.1.0/24,10.0.0.5`); empty means allow-all. The
  listener also reaps idle connections and caps concurrent peers.

## What this folder is NOT

- Not a Modbus *master* — only a slave.
- Not a control loop — control stays in the PLC.
- Not a multi-tenant gateway — assumes one PLC instance per process.
