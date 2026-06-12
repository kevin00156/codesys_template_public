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
	"codesys_dev/backend/internal/modbus"
	"codesys_dev/backend/internal/shm"
	"codesys_dev/backend/internal/state"
	"codesys_dev/backend/internal/wsserver"
	webui "codesys_dev/frontend"
)

func main() {
	var (
		modbusAddr   = flag.String("modbus", ":5020", "Modbus TCP listen address")
		modbusAllow  = flag.String("modbus-allow", "", "comma-separated IPs/CIDRs allowed to reach Modbus (empty = allow all; Modbus has no auth, so restrict to known SCADA hosts in production)")
		httpAddr     = flag.String("http", ":8443", "HTTP/WebSocket listen address")
		tlsCert      = flag.String("tls-cert", "", "TLS certificate file (enables HTTPS; auto-detected from ./cert.pem if empty)")
		tlsKey       = flag.String("tls-key", "", "TLS private key file (auto-detected from ./key.pem if empty)")
		pollInterval = flag.Duration("poll", 10*time.Millisecond, "shm poll interval")
		pushInterval = flag.Duration("push", 100*time.Millisecond, "WebSocket push interval")
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

	// Role-password auth. Hashes come from the environment (all empty => auth
	// disabled, every route open — the dev posture). The session cookie is
	// marked Secure only when we serve TLS. Built here so the same instance gates
	// both the HTTP routes (Wrap) and the WebSocket command plane.
	authn := auth.New(
		os.Getenv("PLC_BRIDGE_PASSWORD_HASH"), // vendor (highest tier)
		os.Getenv("PLC_BRIDGE_TUNER_HASH"),    // tuner
		os.Getenv("PLC_BRIDGE_OPERATOR_HASH"), // operator
		certFile != "",
	)
	if authn.Enabled() {
		log.Printf("auth enabled (vendor/tuner/operator hashes as configured)")
	} else {
		log.Printf("auth disabled — all routes and the command plane are open (set PLC_BRIDGE_*_HASH to enable)")
	}
	if certFile == "" {
		log.Printf("WARNING: serving plain HTTP — login password and session cookie travel in clear. Install cert.pem/key.pem for production.")
	}

	allow, err := modbus.ParseAllowlist(*modbusAllow)
	if err != nil {
		log.Fatalf("modbus-allow: %v", err)
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
	sink := newCmdSink(cmdMap)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		pollShm(ctx, dataMap, snap, *pollInterval)
	}()

	// Modbus TCP server.
	srv := &modbus.Server{Snapshot: snap, Commands: sink, Allow: allow}
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
		Auth:     authn, // the login wall reaches the command plane, not just HTTP
		Interval: *pushInterval,
	}
	go func() {
		if err := serveHTTP(*httpAddr, certFile, keyFile, authn, wsSrv); err != nil {
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

func serveHTTP(addr, certFile, keyFile string, authn *auth.Authenticator, ws *wsserver.Server) error {
	mux := http.NewServeMux()
	mux.Handle("/ws", ws)

	distFS, err := fs.Sub(webui.Files, "dist")
	if err != nil {
		return err
	}
	mux.Handle("/", http.FileServer(http.FS(distFS)))

	authn.RegisterRoutes(mux) // /api/login, /api/logout, /api/auth/status — never gated

	// requiredRole decides the minimum role for an HTTP request. This clean
	// template has no gated HTTP routes yet, so it returns RoleNone (open) for
	// everything and the login flow stays reachable. The command plane (the only
	// write surface that exists) is gated inside wsserver, not here. As machine
	// HTTP APIs are added, gate their writes here, e.g.:
	//
	//	if r.Method != http.MethodGet && strings.HasPrefix(r.URL.Path, "/api/machine/") {
	//		return auth.RoleOperator
	//	}
	requiredRole := func(r *http.Request) auth.Role { return auth.RoleNone }
	handler := securityHeaders(authn.Wrap(mux, requiredRole), certFile != "")

	// Explicit server with timeouts: ReadHeaderTimeout closes the Slowloris
	// slow-header attack, IdleTimeout reaps idle keep-alives. We deliberately
	// leave ReadTimeout/WriteTimeout unset — they would cap the lifetime of the
	// long-lived WebSocket after it hijacks the connection.
	srv := &http.Server{
		Addr:              addr,
		Handler:           handler,
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       120 * time.Second,
	}

	if certFile != "" && keyFile != "" {
		log.Printf("https listening on %s", addr)
		return srv.ListenAndServeTLS(certFile, keyFile)
	}
	log.Printf("http listening on %s", addr)
	return srv.ListenAndServe()
}

// securityHeaders wraps the handler with conservative response headers for the
// embedded SPA. The CSP is intentionally strict (same-origin scripts, no
// framing); style-src allows inline because the build injects scoped styles.
// HSTS is only emitted under TLS — never advertise it over plaintext.
func securityHeaders(next http.Handler, tls bool) http.Handler {
	const csp = "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; " +
		"style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'; frame-ancestors 'none'"
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("X-Frame-Options", "DENY")
		h.Set("Referrer-Policy", "no-referrer")
		h.Set("Content-Security-Policy", csp)
		if tls {
			h.Set("Strict-Transport-Security", "max-age=31536000")
		}
		next.ServeHTTP(w, r)
	})
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

// cmdSink serialises Modbus writes and pushes the latest command to
// /dev/shm/plc_cmd. Atomic-ish semantics: a closure that returns
// non-nil error rolls back any field mutations it made, so a rejected
// partial write never reaches the PLC.
type cmdSink struct {
	mu      sync.Mutex
	mapping *shm.Mapping
	pending shm.PlcCommand
}

func newCmdSink(m *shm.Mapping) *cmdSink {
	c := &cmdSink{mapping: m}
	c.pending.Header.Magic   = shm.PlcCommandMagic
	c.pending.Header.Version = shm.PlcCommandVersion
	return c
}

func (c *cmdSink) Apply(fn func(*shm.PlcCommand) error) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	saved := c.pending
	if err := fn(&c.pending); err != nil {
		c.pending = saved
		return err
	}
	c.pending.Header.Cycle++
	shm.WritePlcCommand(c.mapping, &c.pending)
	return nil
}
