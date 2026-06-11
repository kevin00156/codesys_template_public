#!/usr/bin/env bash
# Wipe stale /dev/shm/plc_* after a shm layout version bump, then bounce the
# CODESYS runtime + plc_bridge so the boot app re-Creates segments at the new
# size.  Symptom that triggers this: plc_bridge journal shows
#     open plc_cmd: /dev/shm/plc_cmd: segment N B, expected at least M B — PLC layout out of date?
#
# Invoked via:  bash -s < scripts/shm-reset.sh   (over ssh or wsl)
set -e

echo "[1/4] stop plc_bridge + codesyscontrol"
sudo -n systemctl stop plc_bridge 2>/dev/null || true
sudo -n systemctl stop codesyscontrol

echo "[2/4] rm /dev/shm/plc_{cmd,data}"
sudo -n rm -f /dev/shm/plc_cmd /dev/shm/plc_data

echo "[3/4] start codesyscontrol, wait for boot app to re-Create shm"
sudo -n systemctl start codesyscontrol
for _ in $(seq 1 30); do
  [ -e /dev/shm/plc_cmd ] && [ -e /dev/shm/plc_data ] && break
  sleep 0.5
done
if [ ! -e /dev/shm/plc_cmd ] || [ ! -e /dev/shm/plc_data ]; then
  echo "ERROR: shm did not reappear within 15s — is the IEC boot app loaded?"
  exit 1
fi
ls -l /dev/shm/plc_cmd /dev/shm/plc_data

echo "[4/4] start plc_bridge"
sudo -n systemctl reset-failed plc_bridge 2>/dev/null || true
sudo -n systemctl start plc_bridge
sleep 1
if sudo -n systemctl is-active plc_bridge >/dev/null; then
  echo "OK"
else
  echo "plc_bridge failed to start — recent journal:"
  sudo -n journalctl -u plc_bridge -n 20 --no-pager
  exit 1
fi
