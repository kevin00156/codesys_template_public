// plc_bridge: read PLC data from /dev/shm, hold a snapshot, serve it
// over Modbus TCP and a WebSocket/HTTP API.
//
// Pin to a non-isolated CPU when running on the CODESYS Edge box:
//
//	taskset -c 0 ./plc_bridge
//
// CPU 2-3 are reserved for the PLC runtime by the kernel cmdline
// (isolcpus=2-3 nohz_full=2-3).
package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"log"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"
	"time"

	"codesys_dev/backend/internal/auth"
	"codesys_dev/backend/internal/cmdsink"
	"codesys_dev/backend/internal/modbus"
	"codesys_dev/backend/internal/shm"
	"codesys_dev/backend/internal/state"
	"codesys_dev/backend/internal/wsserver"
	webui "codesys_dev/frontend"
)

// defaultVendorPassword is the template's built-in fallback login, applied only
// when no PLC_BRIDGE_*_HASH is set in the environment. It exists so a fresh
// clone boots with a known password (the read-only dashboard stays open; only
// machine control needs login) and the UI can nag you to change it.
// CHANGE IT for any real deployment: mint a hash with `plc_bridge -gen-hash`
// and set PLC_BRIDGE_PASSWORD_HASH (see README / docs/DEVELOPMENT.md §4).
const defaultVendorPassword = "111111"

func main() {
	var (
		modbusAddr   = flag.String("modbus", "127.0.0.1:5020", "Modbus TCP listen address; the Modbus write map has no authentication, so bind a non-loopback address (e.g. :5020) only on a firewalled/dedicated machine network")
		httpAddr     = flag.String("http", ":8443", "HTTP/WebSocket listen address")
		tlsCert      = flag.String("tls-cert", "", "TLS certificate file (enables HTTPS; auto-detected from ./cert.pem if empty)")
		tlsKey       = flag.String("tls-key", "", "TLS private key file (auto-detected from ./key.pem if empty)")
		pollInterval = flag.Duration("poll", 10*time.Millisecond, "shm poll interval")
		pushInterval = flag.Duration("push", 100*time.Millisecond, "WebSocket push interval")
		jogTimeout   = flag.Duration("jog-timeout", 500*time.Millisecond, "dead-man timeout: jog bits not refreshed within this window are cleared")
		genHash      = flag.Bool("gen-hash", false, "read a password from stdin, print its bcrypt hash for the PLC_BRIDGE_*_HASH env vars, then exit")
	)
	flag.Parse()

	// Password-hash helper: `plc_bridge -gen-hash` reads one line from stdin and
	// prints a bcrypt hash. Mint each role password this way and store the hash
	// in the service env file — the plaintext never touches args, shell history, or git.
	if *genHash {
		pw, _ := io.ReadAll(os.Stdin)
		h, err := auth.HashPassword(strings.TrimRight(string(pw), "\r\n"))
		if err != nil {
			log.Fatalf("gen-hash: %v", err)
		}
		fmt.Println(h)
		return
	}

	log.SetFlags(log.LstdFlags | log.Lmicroseconds)

	// TLS auto-enables when cert.pem/key.pem sit in the working directory, so a
	// deploy that drops certs in needs no flag change (mirrors the systemd unit).
	certFile, keyFile := *tlsCert, *tlsKey
	if certFile == "" && keyFile == "" && fileExists("cert.pem") && fileExists("key.pem") {
		certFile, keyFile = "cert.pem", "key.pem"
	}

	dataMap, err := shm.OpenRead(shm.NamePlcData, shm.SizePlcData)
	if err != nil {
		log.Fatalf("open %s: %v", shm.NamePlcData, err)
	}
	defer dataMap.Close()

	cmdMap, err := shm.Open(shm.NamePlcCommand, shm.SizePlcCommand)
	if err != nil {
		log.Fatalf("open %s: %v", shm.NamePlcCommand, err)
	}
	defer cmdMap.Close()

	snap := &state.Snapshot{}
	sink := cmdsink.New(func(c *shm.PlcCommand) { shm.WritePlcCommand(cmdMap, c) })

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Dead-man for the level-held jog bits: clients (HMI, Modbus master) must
	// re-send jog commands periodically; bits that stop being refreshed are
	// cleared so a vanished client can't leave an axis moving.
	sink.StartJogWatchdog(ctx, *jogTimeout)

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		pollShm(ctx, dataMap, snap, *pollInterval)
	}()

	// Modbus TCP server.
	srv := &modbus.Server{Snapshot: snap, Commands: sink}
	go func() {
		if err := srv.ListenAndServe(*modbusAddr); err != nil {
			log.Printf("modbus: %v", err)
			cancel()
		}
	}()

	// HTTP/HTTPS server: WebSocket API + embedded frontend.
	wsSrv := &wsserver.Server{
		Snapshot: snap,
		Commands: sink,
		Interval: *pushInterval,
	}
	go func() {
		if err := serveHTTP(*httpAddr, certFile, keyFile, wsSrv); err != nil {
			log.Printf("http: %v", err)
			cancel()
		}
	}()

	proto := "http"
	if certFile != "" {
		proto = "https"
	}
	log.Printf("plc_bridge running. modbus=%s %s=%s poll=%s push=%s",
		*modbusAddr, proto, *httpAddr, *pollInterval, *pushInterval)

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, os.Interrupt, syscall.SIGTERM)
	select {
	case <-sigCh:
		log.Println("signal received, shutting down")
	case <-ctx.Done():
	}
	cancel()
	wg.Wait()
}

func serveHTTP(addr, certFile, keyFile string, ws *wsserver.Server) error {
	mux := http.NewServeMux()
	mux.Handle("/ws", ws)

	distFS, err := fs.Sub(webui.Files, "dist")
	if err != nil {
		return err
	}
	mux.Handle("/", http.FileServer(http.FS(distFS)))

	// Role-password auth. Hashes come from the environment; if none is set the
	// template falls back to a built-in DEFAULT vendor password so a fresh
	// clone runs with a known login and an on-screen "change me" nag. The
	// session cookie is marked Secure only when we serve TLS.
	vendorHash := os.Getenv("PLC_BRIDGE_PASSWORD_HASH")   // vendor (highest tier)
	tunerHash := os.Getenv("PLC_BRIDGE_TUNER_HASH")       // tuner
	operatorHash := os.Getenv("PLC_BRIDGE_OPERATOR_HASH") // operator
	usingDefault := false
	if vendorHash == "" && tunerHash == "" && operatorHash == "" {
		h, err := auth.HashPassword(defaultVendorPassword)
		if err != nil {
			return fmt.Errorf("hash default password: %w", err)
		}
		vendorHash, usingDefault = h, true
	}
	authn := auth.New(vendorHash, tunerHash, operatorHash, certFile != "")
	if usingDefault {
		authn.UseDefaultPassword(defaultVendorPassword) // surfaced via /api/auth/status
	}
	authn.RegisterRoutes(mux) // /api/login, /api/logout, /api/auth/status — never gated

	// Control commands flow over the WebSocket, not HTTP, so writes are gated
	// there: AuthorizeWrite requires a logged-in session of any role while the
	// read-only data push stays open to all. HTTP has no write routes yet, so
	// requiredRole returns RoleNone (everything open) — gate machine HTTP APIs
	// here as you add them, e.g.:
	//
	//	if r.Method != http.MethodGet && strings.HasPrefix(r.URL.Path, "/api/machine/") {
	//		return auth.RoleOperator
	//	}
	ws.AuthorizeWrite = authn.LoggedIn
	requiredRole := func(r *http.Request) auth.Role { return auth.RoleNone }
	handler := authn.Wrap(mux, requiredRole)

	switch {
	case usingDefault:
		log.Printf("auth enabled with built-in DEFAULT password %q — set PLC_BRIDGE_PASSWORD_HASH to change it before production", defaultVendorPassword)
	case authn.Enabled():
		log.Printf("auth enabled (vendor/tuner/operator hashes from env)")
	default:
		log.Printf("auth disabled — all routes open")
	}

	if certFile != "" && keyFile != "" {
		log.Printf("https listening on %s", addr)
		return http.ListenAndServeTLS(addr, certFile, keyFile, handler)
	}
	log.Printf("http listening on %s", addr)
	return http.ListenAndServe(addr, handler)
}

// fileExists reports whether a path exists (used for TLS cert auto-detection).
func fileExists(p string) bool {
	_, err := os.Stat(p)
	return err == nil
}

func pollShm(ctx context.Context, m *shm.Mapping, snap *state.Snapshot, interval time.Duration) {
	tick := time.NewTicker(interval)
	defer tick.Stop()

	var d shm.PlcData
	var lastErr error
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
			err := shm.ReadPlcData(m, &d)
			if err != nil {
				if lastErr == nil || err.Error() != lastErr.Error() {
					log.Printf("read plc_data: %v", err)
				}
				lastErr = err
				continue
			}
			if lastErr != nil {
				log.Println("read plc_data: recovered")
				lastErr = nil
			}
			snap.Update(&d)
		}
	}
}
