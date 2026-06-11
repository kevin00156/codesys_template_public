#!/usr/bin/env bash
# One-time WSL bootstrap for plc_bridge deployment.
#
# Creates everything `make wsl-deploy` assumes already exists:
#   - the plc_bridge service account (-r, nologin)
#   - /opt/plc_bridge install dir
#   - the plc_bridge.service systemd unit (WSL flavour: no CPUAffinity,
#     ordered after codesyscontrol.service instead of codesysedge.service)
#   - /etc/sudoers.d/plc_bridge_deploy so the deploy's `sudo -n` calls
#     (cp / install / systemctl restart) never prompt
#
# Run once from the repo root (prompts for your sudo password):
#     make wsl-bootstrap
#
# Idempotent — safe to re-run after editing the unit or the allowlist.
set -euo pipefail

DEPLOY_USER="${PLC_USER:-plc}"

echo "[bootstrap] service account 'plc_bridge'"
id plc_bridge >/dev/null 2>&1 || sudo useradd -r -s /usr/sbin/nologin plc_bridge

echo "[bootstrap] /opt/plc_bridge"
sudo mkdir -p /opt/plc_bridge
sudo chown plc_bridge:plc_bridge /opt/plc_bridge

echo "[bootstrap] systemd unit /etc/systemd/system/plc_bridge.service"
sudo tee /etc/systemd/system/plc_bridge.service >/dev/null <<'UNIT'
[Unit]
Description=plc_bridge — PLC shm reader + Modbus TCP slave + HTTP API
After=network-online.target codesyscontrol.service
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/plc_bridge
# CODESYS runs as root in WSL, so it Creates /dev/shm/plc_{data,cmd} as
# root:root 0750 — unreadable to this service user, which is why the master
# control reports "PLC 未連線". Widen the segments before the bridge opens them.
# '+' runs this one step with full privileges (User=/NoNewPrivileges/sandbox
# ignored); it re-applies on every start because CODESYS re-Creates the
# segments root:root on each runtime restart. Wait up to 15s for the IEC boot
# app to publish them, then widen (plc_data read-only, plc_cmd read-write).
# Never fails the unit: if CODESYS is down the bridge still serves frontend + CT
# drive, exactly as the available:false degradation intends.
ExecStartPre=+/bin/sh -c 'for i in $(seq 1 30); do [ -e /dev/shm/plc_data ] && [ -e /dev/shm/plc_cmd ] && break; sleep 0.5; done; chmod o+r /dev/shm/plc_data 2>/dev/null || true; chmod o+rw /dev/shm/plc_cmd 2>/dev/null || true'
# Optional auth: drop PLC_BRIDGE_PASSWORD_HASH / PLC_BRIDGE_TUNER_HASH /
# PLC_BRIDGE_OPERATOR_HASH (=<bcrypt hash>, mint with `plc_bridge -gen-hash`)
# into this file to test the login flows locally. Absent => auth disabled.
EnvironmentFile=-/opt/plc_bridge/plc_bridge.env
# TLS auto-enables when cert.pem/key.pem exist here (install with
# `make wsl-deploy-certs`). No certs => plain HTTP, cookie not Secure — fine for
# a local dev box, never for production.
ExecStart=/opt/plc_bridge/plc_bridge --modbus=:5020
Restart=on-failure
RestartSec=2

# Security hardening (mirrors production; CPUAffinity dropped — no isolcpus in WSL)
User=plc_bridge
Group=plc_bridge
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
# /dev/shm for PLC IPC; /opt/plc_bridge for the bridge's default ./orders.db + ./recipes
# (resolved against WorkingDirectory) — without it the service crash-loops on
# "orders db not writable" under ProtectSystem=strict.
ReadWritePaths=/dev/shm /opt/plc_bridge
PrivateTmp=yes
ProtectKernelTunables=yes
RestrictRealtime=yes

[Install]
WantedBy=multi-user.target
UNIT

echo "[bootstrap] sudoers allowlist /etc/sudoers.d/plc_bridge_deploy"
sudo tee /etc/sudoers.d/plc_bridge_deploy.tmp >/dev/null <<EOF
# Companion to /etc/sudoers.d/plc_bridge_shm_reset.
# Lets ${DEPLOY_USER} run the exact commands 'make wsl-deploy' invokes,
# passwordless — no wildcards. Installed by scripts/wsl-bootstrap.sh.
# If you change PLC_USER, re-run 'make wsl-bootstrap'.
${DEPLOY_USER} ALL=(root) NOPASSWD: /usr/bin/cp /opt/plc_bridge/plc_bridge /opt/plc_bridge/plc_bridge.previous
${DEPLOY_USER} ALL=(root) NOPASSWD: /usr/bin/install -m 755 -o plc_bridge -g plc_bridge /tmp/plc_bridge.new /opt/plc_bridge/plc_bridge
# TLS certs for 'make wsl-deploy-certs' — root-owned, plc_bridge-group-readable.
${DEPLOY_USER} ALL=(root) NOPASSWD: /usr/bin/install -m 640 -o root -g plc_bridge /tmp/cert.pem /opt/plc_bridge/cert.pem
${DEPLOY_USER} ALL=(root) NOPASSWD: /usr/bin/install -m 640 -o root -g plc_bridge /tmp/key.pem /opt/plc_bridge/key.pem
${DEPLOY_USER} ALL=(root) NOPASSWD: /usr/bin/systemctl restart plc_bridge
EOF
sudo chown root:root /etc/sudoers.d/plc_bridge_deploy.tmp
sudo chmod 440 /etc/sudoers.d/plc_bridge_deploy.tmp
sudo visudo -cf /etc/sudoers.d/plc_bridge_deploy.tmp
sudo mv /etc/sudoers.d/plc_bridge_deploy.tmp /etc/sudoers.d/plc_bridge_deploy

echo "[bootstrap] daemon-reload + enable (start happens on first deploy)"
sudo systemctl daemon-reload
sudo systemctl enable plc_bridge

echo "[bootstrap] OK — now run: make wsl-deploy"
