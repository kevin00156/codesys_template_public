---
name: deploy-strategy
description: Deployment and development strategy for CODESYS PLC + Go backend + Svelte frontend stack. Use whenever discussing CI/CD, build pipelines, systemd, deployment workflow, or where to run Go / npm builds for this project.
---

# Deployment Strategy: CODESYS + Go + Svelte

## Core rule

**Never build on the industrial PC.** The industrial PC runs CODESYS Edge on a PREEMPT_RT kernel with `isolcpus=2-3`. CPU 0–1 handle all Linux housekeeping. Running `go build`, `npm install`, or VS Code on the PC creates jitter spikes that degrade PLC timing determinism. The PC only ever receives compiled artifacts.

## Three-machine model

```
Dev Machine (laptop)          Build / CI (optional)       Industrial PC
─────────────────────         ──────────────────────      ────────────────────────
git repo (source of truth)    GitHub Actions or           PLC runtime (CODESYS)
CODESYS IDE                   self-hosted runner          plc_bridge binary
Go toolchain                  GOOS=linux GOARCH=amd64     /opt/plc_bridge/
Node / pnpm                   pnpm build                  systemd units only
VSCode                        publish artifacts
```

For small teams without a CI server, the dev machine doubles as the build machine. The industrial PC role never changes.

## Per-tier deployment

### PLC (CODESYS)

- **Source of truth**: `Device/` export tree committed to git.
- **Deploy path**: CODESYS IDE → Online → Login → Download via CODESYS gateway (`codesysedge.bin` on the industrial PC).
- **Interruption**: Download triggers warm/cold reset — only in scheduled maintenance windows.
- **No-interrupt update**: Online Change (structural changes not allowed).
- **Rollback**: `git checkout <tag>`, re-download via IDE.
- **Version bump trigger**: Any change to IEC DUT layout that is visible to Go (`PlcData`, `PlcCommand`) — must bump `PLC_DATA_VERSION` / `PLC_CMD_VERSION` in both `codesys_export/Device/Application/Programs/PRG_ShmPublisher.st` and `backend/internal/shm/layout.go`.

### Go backend (`plc_bridge`)

Build on dev machine:
```bash
GOOS=linux GOARCH=amd64 go build -trimpath -ldflags="-s -w" \
    -o backend/dist/plc_bridge ./backend/cmd/plc_bridge
```

Deploy:
```bash
rsync -av backend/dist/plc_bridge $REMOTE:/opt/plc_bridge/plc_bridge.new
ssh $REMOTE "sudo install -m 755 -o plc_bridge -g plc_bridge \
    /opt/plc_bridge/plc_bridge.new /opt/plc_bridge/plc_bridge && \
    sudo systemctl restart plc_bridge"
```

- Single static binary, no runtime dependencies, no glibc version issues.
- Restart takes ~1 s. Modbus master must reconnect. Process is stateless.
- Keep previous binary as `/opt/plc_bridge/plc_bridge.previous` for one-command rollback.

### Svelte frontend

Build on dev machine and embed into the Go binary:

```go
// in cmd/plc_bridge/main.go
//go:embed frontend/dist
var webFS embed.FS
```

```bash
cd frontend && pnpm build        # outputs to frontend/dist/
go build ./plc_bridge/...   # embeds dist/ at compile time
```

Deployed as part of the same binary update as the Go backend — frontend and API are always in sync by construction. No nginx, no Node runtime on the industrial PC.

During development only: run `vite dev` on the dev machine with a proxy pointing to the Go API on the industrial PC.

## systemd unit

`/etc/systemd/system/plc_bridge.service`:

```ini
[Unit]
Description=plc_bridge — PLC shm reader + Modbus TCP slave + HTTP API
After=network-online.target codesysedge.service
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/plc_bridge/plc_bridge --modbus=:5020
Restart=on-failure
RestartSec=2

# Pin to housekeeping cores; PLC owns 2-3 via isolcpus
CPUAffinity=0 1

# Security hardening
User=plc_bridge
Group=plc_bridge
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/dev/shm
PrivateTmp=yes
ProtectKernelTunables=yes
RestrictRealtime=yes

[Install]
WantedBy=multi-user.target
```

Setup:
```bash
sudo useradd -r -s /usr/sbin/nologin plc_bridge
sudo mkdir -p /opt/plc_bridge
sudo systemctl daemon-reload
sudo systemctl enable --now plc_bridge
```

## Makefile (dev machine)

```makefile
REMOTE  ?= plc@industrial-pc
GOOS    := linux
GOARCH  := amd64

.PHONY: build deploy logs rollback

build:
	cd frontend && pnpm build
	GOOS=$(GOOS) GOARCH=$(GOARCH) \
	    go build -trimpath -ldflags="-s -w" \
	    -o backend/dist/plc_bridge ./backend/cmd/plc_bridge

deploy: build
	rsync -av --progress backend/dist/plc_bridge \
	    $(REMOTE):/opt/plc_bridge/plc_bridge.new
	ssh $(REMOTE) "sudo cp /opt/plc_bridge/plc_bridge \
	                       /opt/plc_bridge/plc_bridge.previous 2>/dev/null; \
	               sudo install -m 755 -o plc_bridge -g plc_bridge \
	                   /opt/plc_bridge/plc_bridge.new \
	                   /opt/plc_bridge/plc_bridge && \
	               sudo systemctl restart plc_bridge"

rollback:
	ssh $(REMOTE) "sudo cp /opt/plc_bridge/plc_bridge.previous \
	                       /opt/plc_bridge/plc_bridge && \
	               sudo systemctl restart plc_bridge"

logs:
	ssh $(REMOTE) "journalctl -u plc_bridge -f -n 100"
```

## Mock PLC for offline development

When the industrial PC is unavailable, run a mock publisher on the dev machine that creates the same `/dev/shm` segments as the real PLC:

`cmd/mock_plc/main.go` — writes synthetic telemetry (sine wave) at 10 ms, reads commands from `plc_cmd` and echoes setpoints back into the telemetry. Lets you develop and test the entire Go + Svelte stack without a CODESYS licence or industrial hardware.

## Key disciplines

| Rule | Reason |
|---|---|
| Never `go build` or `npm install` on the industrial PC | CPU jitter degrades PLC timing |
| Build is automatic, deploy is manual | Industrial machines are not web services; auto-deploy is an incident waiting to happen |
| One git tag covers all three tiers simultaneously | Prevents "which version of what is running" debugging sessions |
| PLC layout version and Go layout version must match at all times | Mismatch = magic check fails loudly, not silently corrupt |
| Keep previous binary on the PC | One-command rollback without internet access |
| `/dev/shm` names must not collide with CODESYS internals | `GWDrvSharedMemShm`, `CME-*`, `sem.*` are taken; use your own prefix |
| `taskset -c 0 ./plc_bridge` if running manually (without systemd CPUAffinity) | Keeps Go process off isolated cores 2–3 |

## Versioning convention

Use date-based tags: `v2026.05.01`. Tag after all three tiers are in sync:

```bash
git tag v2026.05.01 -m "PLC v3 / bridge v2 / web v1"
git push origin v2026.05.01
```

PLC boot application and Go binary both carry version identifiers (magic + version field in shm). If they disagree on startup, `plc_bridge` logs `ErrVersionMismatch` and refuses to serve data, making the mismatch immediately visible in `journalctl`.
