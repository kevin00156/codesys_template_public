-include .env
export PLC_HOST PLC_USER

PLC_HOST   ?= 192.168.1.10
PLC_USER   ?= plc
REMOTE     ?= $(PLC_USER)@$(PLC_HOST)
WSL_DISTRO ?= Ubuntu-24.04
GOOS       := linux
GOARCH     := amd64
GO         ?= go
BIN        := backend/dist/plc_bridge

.PHONY: build frontend deploy wsl-deploy wsl-bootstrap shm-reset wsl-shm-reset setup-sudoers wsl-setup-sudoers logs rollback clean cert deploy-certs wsl-deploy-certs test vet

frontend:
	cd frontend && npm run build

# Unit tests + vet. The backend compiles on the host now (the /dev/shm mmap
# lives behind a `//go:build linux` tag), so these run natively on the dev
# box — no WSL round-trip needed. Cross-compile is still checked by `build`.
test:
	$(GO) test ./backend/...

vet:
	$(GO) vet ./backend/...
	GOOS=$(GOOS) GOARCH=$(GOARCH) $(GO) vet ./backend/...

build: frontend
	mkdir -p backend/dist
	GOOS=$(GOOS) GOARCH=$(GOARCH) \
	    $(GO) build -trimpath -ldflags="-s -w" \
	    -o $(BIN) ./backend/cmd/plc_bridge

deploy: build
	scp $(BIN) $(REMOTE):/tmp/plc_bridge.new
	ssh $(REMOTE) "sudo -n cp /opt/plc_bridge/plc_bridge \
	                       /opt/plc_bridge/plc_bridge.previous 2>/dev/null; \
	               sudo -n install -m 755 -o plc_bridge -g plc_bridge \
	                   /tmp/plc_bridge.new /opt/plc_bridge/plc_bridge && \
	               sudo -n systemctl restart plc_bridge && \
	               rm -f /tmp/plc_bridge.new"

# One-time WSL setup: creates the plc_bridge user, /opt/plc_bridge, the systemd
# unit, and the passwordless-sudo allowlist that wsl-deploy's `sudo -n` calls
# need. Streamed via stdin (like shm-reset); prompts once for your sudo password.
# Run this before the first `make wsl-deploy` on a fresh WSL distro.
wsl-bootstrap:
	wsl -d $(WSL_DISTRO) -- bash -s < scripts/wsl-bootstrap.sh

# WSL fallback path — same build, same install layout, no scp.
# Prerequisites in the WSL distro: /opt/plc_bridge/ exists, plc_bridge user/group
# exists, systemd unit plc_bridge.service is installed, and `sudo -n` works for plc.
#
# Path translation note: `wslpath` is a /init symlink (WSL interop) and does NOT
# write to a pipe inside command substitution, so we translate Make's $(CURDIR)
# (e.g. "C:/Users/.../codesys_dev") to "/mnt/c/Users/.../codesys_dev" with patsubst.
# Assumes the repo is on C:. Override WSL_BIN_PATH explicitly if on another drive.
WSL_BIN_PATH ?= /mnt/c$(patsubst C:%,%,$(CURDIR))/$(BIN)

wsl-deploy: build
	wsl -d $(WSL_DISTRO) -- bash -c 'set -e; \
	  cp "$(WSL_BIN_PATH)" /tmp/plc_bridge.new; \
	  sudo -n cp /opt/plc_bridge/plc_bridge /opt/plc_bridge/plc_bridge.previous 2>/dev/null || true; \
	  sudo -n install -m 755 -o plc_bridge -g plc_bridge /tmp/plc_bridge.new /opt/plc_bridge/plc_bridge; \
	  sudo -n systemctl restart plc_bridge; \
	  rm -f /tmp/plc_bridge.new; \
	  echo OK'

# Wipe stale /dev/shm/plc_* after a shm layout version bump, then bounce the stack.
# Body lives in scripts/shm-reset.sh; both targets stream it via stdin so the
# host shell never has to expand a multi-line variable (PowerShell/cmd/sh have
# inconsistent semantics for that and corrupt the recipe).
shm-reset:
	ssh $(REMOTE) bash -s < scripts/shm-reset.sh

wsl-shm-reset:
	wsl -d $(WSL_DISTRO) -- bash -s < scripts/shm-reset.sh

# One-shot: install scripts/shm-reset.sudoers as /etc/sudoers.d/plc_bridge_shm_reset
# so shm-reset.sh's `sudo -n` calls don't prompt for password.  Prompts once
# for the user's sudo password.  Validates with visudo -c before activating.
setup-sudoers:
	ssh -t $(REMOTE) "sudo install -m 440 -o root -g root /dev/stdin /etc/sudoers.d/plc_bridge_shm_reset.tmp && sudo visudo -cf /etc/sudoers.d/plc_bridge_shm_reset.tmp && sudo mv /etc/sudoers.d/plc_bridge_shm_reset.tmp /etc/sudoers.d/plc_bridge_shm_reset && echo OK" < scripts/shm-reset.sudoers

wsl-setup-sudoers:
	wsl -d $(WSL_DISTRO) -- bash -c "sudo install -m 440 -o root -g root /dev/stdin /etc/sudoers.d/plc_bridge_shm_reset.tmp && sudo visudo -cf /etc/sudoers.d/plc_bridge_shm_reset.tmp && sudo mv /etc/sudoers.d/plc_bridge_shm_reset.tmp /etc/sudoers.d/plc_bridge_shm_reset && echo OK" < scripts/shm-reset.sudoers

# Generate self-signed TLS cert with SAN baked from PLC_HOST.
# Output: certs/cert.pem certs/key.pem (consumed by deploy-certs).
# Uses our own minimal -config (scripts/openssl-req.cnf) so generation does not
# depend on a system openssl.cnf — that default is absent on Windows/conda
# openssl. Output goes to a repo-local dir (gitignored), not /tmp, because a
# native-Windows openssl can't write the MSYS /tmp path.
cert:
	mkdir -p certs
	openssl req -x509 -newkey rsa:2048 -keyout certs/key.pem -out certs/cert.pem \
	    -days 3650 -nodes -config scripts/openssl-req.cnf \
	    -addext "subjectAltName=IP:127.0.0.1,IP:$(PLC_HOST),DNS:localhost"

# Copy TLS certs to remote (run once after generating certs).
# Owned root:plc_bridge 0640 so the service (Group=plc_bridge) can read key.pem
# while it stays unreadable to everyone else.
deploy-certs:
	scp certs/cert.pem certs/key.pem $(REMOTE):/tmp/
	ssh $(REMOTE) "sudo -n cp /tmp/cert.pem /tmp/key.pem /opt/plc_bridge/ && \
	               sudo -n chown root:plc_bridge /opt/plc_bridge/key.pem /opt/plc_bridge/cert.pem && \
	               sudo -n chmod 640 /opt/plc_bridge/key.pem /opt/plc_bridge/cert.pem && \
	               rm -f /tmp/cert.pem /tmp/key.pem"

# WSL fallback for deploy-certs — same install layout, no scp (the repo is
# already visible inside WSL under /mnt/c). Uses the exact `install` commands the
# wsl-bootstrap sudoers allowlist grants; re-run `make wsl-bootstrap` first if
# you haven't since this target was added (it adds the cert entries). Restarts
# the service so TLS auto-enables once the certs land.
WSL_CERTS_DIR ?= /mnt/c$(patsubst C:%,%,$(CURDIR))/certs
wsl-deploy-certs:
	wsl -d $(WSL_DISTRO) -- bash -c 'set -e; \
	  cp "$(WSL_CERTS_DIR)/cert.pem" "$(WSL_CERTS_DIR)/key.pem" /tmp/; \
	  sudo -n install -m 640 -o root -g plc_bridge /tmp/cert.pem /opt/plc_bridge/cert.pem; \
	  sudo -n install -m 640 -o root -g plc_bridge /tmp/key.pem /opt/plc_bridge/key.pem; \
	  rm -f /tmp/cert.pem /tmp/key.pem; \
	  sudo -n systemctl restart plc_bridge; \
	  echo OK'

rollback:
	ssh $(REMOTE) "sudo -n cp /opt/plc_bridge/plc_bridge.previous \
	                       /opt/plc_bridge/plc_bridge && \
	               sudo -n systemctl restart plc_bridge"

logs:
	ssh $(REMOTE) "journalctl -u plc_bridge -f -n 100"

clean:
	rm -rf backend/dist frontend/dist
