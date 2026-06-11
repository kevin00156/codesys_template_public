---
name: wsl-deploy-strategy
description: Local WSL2 deployment strategy for end-to-end logic validation of the CODESYS + Go + Svelte stack. Covers installing CODESYS Control Linux SL into WSL, building Go/frontend natively in WSL, the collapsed single-machine model, demo-license caveats, and what is and is not testable. Use whenever the user is setting up, troubleshooting, or extending the WSL test environment, or asking how to deploy the project locally instead of to the industrial PC.
---

# WSL2 Local Deployment Strategy

This skill covers the **logic-validation-only** path: run everything inside one WSL2 Ubuntu-22.04 distro on the dev laptop. For the production path to the industrial PC, see [[deploy-strategy]]. For the WSL environment snapshot, see [[wsl-system-info]].

## Core rule

**WSL is for logic, not for timing.** Anything that depends on PREEMPT_RT, `isolcpus`, deterministic cycle time, or hard deadlines is meaningless in WSL. Anything that depends on state machine behavior, cam math, FB return codes, shm layout, Modbus map, or HMI flows is identical to the industrial PC.

Do not promote a "passes in WSL" result to "ready for the industrial PC" without re-running it on real hardware.

## Collapsed single-machine model

The production model has three roles (dev / build / industrial PC). In WSL, all three collapse into one:

```
Windows host                      WSL2 Ubuntu-22.04
─────────────                     ────────────────────────────────────
CODESYS IDE                       CODESYS Control Linux SL (demo)
Browser                           plc_bridge (Go binary)
Code editor (if you prefer)       /dev/shm/plc_data, /dev/shm/plc_cmd
                                  Source tree, Go toolchain, pnpm
        │
        └── localhost:8443 ─────► forwarded into WSL automatically
            localhost:5020         (Modbus TCP)
            localhost:11740/41     (CODESYS gateway, for IDE online)
```

One git repo, cloned into WSL ext4. Both build and run happen there. The Windows side is only the IDE and the browser.

## One-time setup

### 1. Confirm WSL distro

```powershell
# Windows PowerShell
wsl --list --verbose
# expect: Ubuntu-22.04 ... 2 (Running or Stopped)
wsl -d Ubuntu-22.04
```

If `systemd=true` is not in `/etc/wsl.conf`, add it under `[boot]` and `wsl --shutdown` once.

### 2. Clone the repo into WSL ext4 (NOT `/mnt/c`)

```bash
# inside WSL
cd ~
git clone <repo-url> codesys_dev
cd codesys_dev
```

Working from `/mnt/c/Users/...` is unusably slow over 9P. Always use the WSL-native filesystem.

### 3. Install Go and pnpm

```bash
# Go (matches GOOS/GOARCH the production deploy uses)
GO_VER=1.23.4
curl -L "https://go.dev/dl/go${GO_VER}.linux-amd64.tar.gz" \
  | sudo tar -C /usr/local -xz
echo 'export PATH=$PATH:/usr/local/go/bin' >> ~/.bashrc
. ~/.bashrc
go version

# Node + pnpm via nvm (so they live under $HOME, not /mnt/c)
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
. ~/.bashrc
nvm install --lts
corepack enable
corepack prepare pnpm@latest --activate
pnpm --version
```

Pin versions in the repo's tooling files if/when you settle on specific releases.

### 4. Install CODESYS Control Linux SL

CODESYS Store package: **CODESYS Control for Linux SL** (`.package` for IDE-side, plus a `.deb`/tarball for the Linux runtime).

```bash
# Example — substitute the actual version you downloaded
sudo dpkg -i codesyscontrol-linux_4.13.0.0_amd64.deb
# or for tarball variants:
# sudo tar -C / -xzf codesyscontrol-linux-*.tar.gz
```

Install paths follow the same layout as Edge:
- `/opt/codesys/` — libraries
- Runtime binary typically at `/opt/codesys/bin/codesyscontrol` (verify with `dpkg -L`)
- Config: `/etc/codesyscontrol/CODESYSControl.cfg`

**License model**: demo runtime runs in **2-hour windows** without a paid license. SoftMotion is included in demo too. Plan tests around the 2-hour cap, or buy a developer SL license bound to a virtual machine ID.

### 5. Wire up systemd units (optional but recommended)

`/etc/systemd/system/codesyscontrol.service`:

```ini
[Unit]
Description=CODESYS Control Linux SL runtime (demo / dev use only)
After=network-online.target

[Service]
Type=simple
ExecStart=/opt/codesys/bin/codesyscontrol
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
```

For `plc_bridge`, reuse the unit from [[deploy-strategy]] but drop `CPUAffinity` — there are no isolated cores to avoid in WSL.

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now codesyscontrol plc_bridge
```

Quick smoke run without systemd is also fine for ad-hoc testing:

```bash
sudo /opt/codesys/bin/codesyscontrol &       # publishes shm once a boot app runs
go run ./backend/cmd/plc_bridge
```

## Daily workflow

### Build

Native Linux build inside WSL. Identical commands to the production build, no cross-compile needed:

```bash
cd ~/codesys_dev
cd frontend && pnpm install && pnpm build && cd ..
go build -trimpath -ldflags="-s -w" \
    -o backend/dist/plc_bridge ./backend/cmd/plc_bridge
```

A WSL-flavored Makefile target can wrap this — different from the production Makefile because there is no remote `rsync` step:

```makefile
.PHONY: wsl-build wsl-run wsl-stop

wsl-build:
	cd frontend && pnpm build
	go build -trimpath -ldflags="-s -w" \
	    -o backend/dist/plc_bridge ./backend/cmd/plc_bridge

wsl-run: wsl-build
	./backend/dist/plc_bridge --modbus=:5020 --http=:8443

wsl-stop:
	pkill -x plc_bridge || true
```

### Deploy the IEC project to the WSL runtime

1. Open the CODESYS IDE on Windows.
2. Add a gateway pointing to `localhost` (WSL forwards 11740/11741 to the host).
3. Select the Control Linux SL device in the project tree, scan network, pick the WSL instance.
4. Login → Download → Run. Same flow as the industrial PC, just a different device target.
5. `PRG_ShmPublisher` starts populating `/dev/shm/plc_data` and reading `/dev/shm/plc_cmd`.

If gateway discovery fails, check that the WSL runtime is actually listening: `ss -tlnp | grep -E '11740|11741'` inside WSL.

### Verify the loop

```bash
# inside WSL
ls -la /dev/shm/plc_data /dev/shm/plc_cmd          # both should exist, sized per layout
xxd /dev/shm/plc_data | head -2                    # magic bytes 'DCLP' (little-endian 'PLCD')
journalctl -u plc_bridge -f                        # or run in foreground
```

```powershell
# Windows browser
start http://localhost:8443
```

### Iterate

- IEC changes → CODESYS IDE → Online Change (for non-structural) or Login + Download.
- Go changes → `go build` + restart `plc_bridge` (`sudo systemctl restart plc_bridge` or kill the foreground process).
- Frontend changes → `pnpm build` then rebuild Go (because the dist is embedded), **or** during heavy frontend work run `pnpm dev` on a separate port with the bridge as proxy target.
- **Layout changes** (any `PlcData`/`PlcCommand` field add/remove): bump `PLC_DATA_VERSION` / `PLC_CMD_VERSION` in both [PRG_ShmPublisher.st](codesys_export/Device/Application/Programs/PRG_ShmPublisher.st) and [layout.go](backend/internal/shm/layout.go). Mismatch will fail loudly via `ErrVersionMismatch`.

## Mock PLC alternative (no CODESYS at all)

If you only want to develop the frontend or the bridge plumbing and not the IEC logic, skip CODESYS Control Linux SL entirely. Write or run `cmd/mock_plc/main.go` (per [[deploy-strategy]]) — it creates the same `/dev/shm` segments with synthetic telemetry. No 2-hour cap, no license, fast iteration. Use this when you are not validating motion logic.

```bash
go run ./backend/cmd/mock_plc &
go run ./backend/cmd/plc_bridge
```

## Demo runtime survival tips

- 2-hour cap kicks in silently — set a timer or check `journalctl -u codesyscontrol` for the warning.
- `sudo systemctl restart codesyscontrol` resets the clock. The IEC application reloads from boot app; retain variables reset unless you've configured persistent retain.
- For tests that need more than 2 hours, split into segments and verify state continuity manually, or get a paid SL dev license.
- Some SoftMotion library features may be flagged demo-only (limited cam table size, axis count) — read the warning banner the IDE shows at first login.

## Key disciplines (WSL-specific)

| Rule | Reason |
|---|---|
| Never put the working tree under `/mnt/c/...` | 9P filesystem is 10-100× slower than ext4; `go build` and `pnpm install` become painful |
| Drop `CPUAffinity` from systemd units | No isolated cores in WSL — pinning has no effect or hurts |
| Treat WSL IP as ephemeral | Resets across `wsl --shutdown`; never hard-code into configs |
| Use `localhost:<port>` from Windows | Win11 auto-forwards into WSL; manual port-proxy is for LAN access only |
| Wipe `/dev/shm` between scenarios | `sudo rm /dev/shm/plc_data /dev/shm/plc_cmd` clears stale state if you skip a runtime restart |
| Don't trust timing numbers | Cycle jitter is meaningless here — verify timing only on the industrial PC |
| Re-bump shm version constants on both sides | Same rule as production; the mismatch check is your safety net |

## Promotion path WSL → industrial PC

When WSL logic tests are green and you want to deploy to the industrial PC:

1. Commit the IEC export + Go + frontend together (matching versions).
2. Tag: `git tag v2026.05.15`.
3. On the dev laptop (Windows or WSL), build the production binary with `make build` from [[deploy-strategy]] — same Go source, same `GOOS=linux GOARCH=amd64`, identical artifact.
4. `make deploy` rsyncs to the industrial PC and restarts the service.
5. Re-run timing-sensitive tests on the industrial PC — those were skipped in WSL.

The IEC application is deployed via the IDE the same way as in WSL, but the target device is the industrial PC's gateway instead of `localhost`.
