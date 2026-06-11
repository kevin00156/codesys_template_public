// Package auth gates routes behind three role passwords:
//
//   - operator (操作員): may command the routine operator surfaces (machine
//     start/stop and the like). Opt-in: with no operator password configured
//     those surfaces stay open, preserving simpler two-tier deployments.
//   - tuner  (調機): everything operator has, plus the tuning / debug surfaces
//     — but not the configuration ones.
//   - vendor (廠商): full access — everything tuner has, plus parameters and
//     machine configuration.
//
// The data model stays deliberately small: no user accounts, just one bcrypt
// hash per role and a set of live session tokens (token → role) in memory. A
// process restart drops every session — an HMI has no durable-session
// requirement, operators simply log in again. Hashes come from environment
// variables (PLC_BRIDGE_PASSWORD_HASH = vendor, kept for back-compat;
// PLC_BRIDGE_TUNER_HASH = tuner; PLC_BRIDGE_OPERATOR_HASH = operator) so they
// never land in process args, shell history, or git. Login is a single
// password field: it is matched against vendor first, then tuner, then
// operator — the password decides the role.
//
// Enforcement is one chokepoint: Wrap() guards every request via a
// caller-supplied requiredRole(path) predicate. Hiding tabs in the frontend is
// cosmetic; this is the wall.
//
// Endpoints (registered unprotected, so the login screen can reach them):
//
//	POST /api/login        {password}  -> sets an HttpOnly session cookie
//	POST /api/logout                   -> clears it
//	GET  /api/auth/status              -> {enabled, loggedIn, role}
package auth

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net"
	"net/http"
	"strconv"
	"sync"
	"time"

	"golang.org/x/crypto/bcrypt"
)

const (
	cookieName = "plc_session"
	sessionTTL = 12 * time.Hour

	// Login throttle. bcrypt is deliberately slow, so an unauthenticated caller
	// can both brute-force passwords and amplify a few requests into CPU
	// exhaustion (one bcrypt per attempt). After maxLoginFails wrong passwords
	// from one client IP we lock that IP out for loginLockout before trying
	// another hash. maxLoginBytes caps the request body so a giant payload can't
	// balloon memory before we even look at it.
	maxLoginFails = 5
	loginLockout  = 1 * time.Minute
	maxLoginBytes = 4 << 10 // 4 KiB
)

// Role is a session privilege level, strictly ordered:
// vendor ⊇ tuner ⊇ operator.
type Role string

const (
	RoleNone     Role = ""         // not logged in / open route
	RoleOperator Role = "operator" // 操作員
	RoleTuner    Role = "tuner"    // 調機
	RoleVendor   Role = "vendor"   // 廠商 (full access)
)

// Satisfies reports whether a session role meets a route requirement.
func (r Role) Satisfies(required Role) bool {
	switch required {
	case RoleNone:
		return true
	case RoleOperator:
		return r != RoleNone // any logged-in role may operate
	case RoleTuner:
		return r == RoleTuner || r == RoleVendor
	default: // RoleVendor
		return r == RoleVendor
	}
}

type session struct {
	role Role
	exp  time.Time
}

// Authenticator holds the per-role password hashes and the live session set.
// The zero value is not usable; construct with New.
type Authenticator struct {
	vendorHash   []byte // bcrypt; all three nil => auth disabled
	tunerHash    []byte
	operatorHash []byte // nil => operator-tier routes stay open (back-compat)
	secure       bool   // mark the session cookie Secure (HTTPS only)

	mu     sync.Mutex
	tokens map[string]session

	loginMu   sync.Mutex
	loginFail map[string]loginAttempt // client IP -> recent failure state
}

// loginAttempt tracks one client IP's recent failed logins. While now < until
// the IP is locked out and no bcrypt comparison runs for it.
type loginAttempt struct {
	fails int
	until time.Time
}

// New builds an Authenticator. Empty hashes disable auth entirely: every
// route is open and /api/auth/status reports enabled:false — the intended
// posture on a dev box. secure should be true when serving TLS so the cookie
// is never sent in clear.
func New(vendorHash, tunerHash, operatorHash string, secure bool) *Authenticator {
	a := &Authenticator{secure: secure, tokens: map[string]session{}, loginFail: map[string]loginAttempt{}}
	if vendorHash != "" {
		a.vendorHash = []byte(vendorHash)
	}
	if tunerHash != "" {
		a.tunerHash = []byte(tunerHash)
	}
	if operatorHash != "" {
		a.operatorHash = []byte(operatorHash)
	}
	return a
}

// Enabled reports whether any password is configured. When false the
// Authenticator lets everything through.
func (a *Authenticator) Enabled() bool {
	return a.vendorHash != nil || a.tunerHash != nil || a.operatorHash != nil
}

// OperatorGated reports whether the operator tier is active. Routes requiring
// RoleOperator stay open while it is false, so deployments that only ever
// configured the vendor/tuner passwords keep their HMI operator surfaces
// working exactly as before.
func (a *Authenticator) OperatorGated() bool { return a.operatorHash != nil }

// HashPassword returns a bcrypt hash suitable for the *_HASH env vars.
// Used by the plc_bridge -gen-hash helper.
func HashPassword(pw string) (string, error) {
	h, err := bcrypt.GenerateFromPassword([]byte(pw), bcrypt.DefaultCost)
	return string(h), err
}

// RegisterRoutes mounts the login/logout/status endpoints. These must never be
// gated by Wrap's predicate, or the login screen can't reach them.
func (a *Authenticator) RegisterRoutes(mux *http.ServeMux) {
	mux.HandleFunc("POST /api/login", a.handleLogin)
	mux.HandleFunc("POST /api/logout", a.handleLogout)
	mux.HandleFunc("GET /api/auth/status", a.handleStatus)
}

// Wrap returns a handler that rejects any request whose requiredRole(r) the
// session does not satisfy. With auth disabled it is a pass-through. The
// predicate sees the whole request so it can gate by method as well as path
// (operator surfaces gate writes only — reads feed the always-on dashboard).
func (a *Authenticator) Wrap(next http.Handler, requiredRole func(r *http.Request) Role) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if a.Enabled() {
			req := requiredRole(r)
			if req == RoleOperator && !a.OperatorGated() {
				req = RoleNone // operator tier not configured — stays open
			}
			if req != RoleNone && !a.sessionRole(r).Satisfies(req) {
				writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "需要登入"})
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

// Allows reports whether the request's session may perform an action that
// requires `required`. It applies the same policy as Wrap — auth disabled lets
// everything through, and the operator tier downgrades to open when no operator
// hash is configured — so non-HTTP surfaces (the WebSocket command plane) gate
// on identical rules. The whole point: the login wall must protect the control
// path, not just HTTP routes.
func (a *Authenticator) Allows(r *http.Request, required Role) bool {
	if !a.Enabled() || required == RoleNone {
		return true
	}
	if required == RoleOperator && !a.OperatorGated() {
		return true // operator tier not configured — stays open (back-compat)
	}
	return a.sessionRole(r).Satisfies(required)
}

// SessionRole exposes the live session role for a request (RoleNone when not
// logged in; RoleVendor when auth is disabled). Used for audit logging on the
// command plane.
func (a *Authenticator) SessionRole(r *http.Request) Role { return a.sessionRole(r) }

func (a *Authenticator) handleStatus(w http.ResponseWriter, r *http.Request) {
	role := a.sessionRole(r)
	writeJSON(w, http.StatusOK, map[string]any{
		"enabled":       a.Enabled(),
		"loggedIn":      role != RoleNone || !a.Enabled(),
		"role":          string(role),
		"operatorGated": a.OperatorGated(), // mirror for the frontend's tab locks
	})
}

func (a *Authenticator) handleLogin(w http.ResponseWriter, r *http.Request) {
	if !a.Enabled() {
		// Nothing to authenticate against — report success so the UI proceeds.
		writeJSON(w, http.StatusOK, map[string]any{"ok": true, "role": string(RoleVendor)})
		return
	}
	now := time.Now()
	ip := clientIP(r)
	if locked, retry := a.loginThrottled(ip, now); locked {
		w.Header().Set("Retry-After", strconv.Itoa(int(retry.Seconds())+1))
		writeJSON(w, http.StatusTooManyRequests, map[string]string{"error": "嘗試次數過多，請稍後再試"})
		return
	}
	// Cap the body before decoding so an oversized payload can't balloon memory.
	r.Body = http.MaxBytesReader(w, r.Body, maxLoginBytes)
	var body struct {
		Password string `json:"password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "bad json"})
		return
	}
	// The password decides the role: vendor, then tuner, then operator. bcrypt's
	// compare is constant-time per hash; all mismatching lands in one generic error.
	role := RoleNone
	if a.vendorHash != nil && bcrypt.CompareHashAndPassword(a.vendorHash, []byte(body.Password)) == nil {
		role = RoleVendor
	} else if a.tunerHash != nil && bcrypt.CompareHashAndPassword(a.tunerHash, []byte(body.Password)) == nil {
		role = RoleTuner
	} else if a.operatorHash != nil && bcrypt.CompareHashAndPassword(a.operatorHash, []byte(body.Password)) == nil {
		role = RoleOperator
	}
	a.recordLogin(ip, role != RoleNone, now)
	if role == RoleNone {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "密碼錯誤"})
		return
	}
	http.SetCookie(w, &http.Cookie{
		Name:     cookieName,
		Value:    a.newToken(role),
		Path:     "/",
		HttpOnly: true,
		Secure:   a.secure,
		SameSite: http.SameSiteStrictMode,
		MaxAge:   int(sessionTTL.Seconds()),
	})
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "role": string(role)})
}

func (a *Authenticator) handleLogout(w http.ResponseWriter, r *http.Request) {
	if c, err := r.Cookie(cookieName); err == nil {
		a.mu.Lock()
		delete(a.tokens, c.Value)
		a.mu.Unlock()
	}
	http.SetCookie(w, &http.Cookie{
		Name:     cookieName,
		Value:    "",
		Path:     "/",
		HttpOnly: true,
		Secure:   a.secure,
		SameSite: http.SameSiteStrictMode,
		MaxAge:   -1,
	})
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

// loginThrottled reports whether ip is currently locked out, and for how long.
func (a *Authenticator) loginThrottled(ip string, now time.Time) (bool, time.Duration) {
	a.loginMu.Lock()
	defer a.loginMu.Unlock()
	at, ok := a.loginFail[ip]
	if ok && now.Before(at.until) {
		return true, at.until.Sub(now)
	}
	return false, 0
}

// recordLogin updates the failure counter for ip. A success clears it; the
// maxLoginFails-th failure arms a loginLockout window. It also opportunistically
// drops stale entries so the map can't grow without bound.
func (a *Authenticator) recordLogin(ip string, ok bool, now time.Time) {
	a.loginMu.Lock()
	defer a.loginMu.Unlock()
	if ok {
		delete(a.loginFail, ip)
		return
	}
	at := a.loginFail[ip]
	at.fails++
	if at.fails >= maxLoginFails {
		at.until = now.Add(loginLockout)
		at.fails = 0 // counter resets; the lock window now does the gating
	}
	a.loginFail[ip] = at
	for k, v := range a.loginFail {
		if v.fails == 0 && now.After(v.until) {
			delete(a.loginFail, k)
		}
	}
}

// clientIP extracts the host portion of r.RemoteAddr for throttle keying.
func clientIP(r *http.Request) string {
	if host, _, err := net.SplitHostPort(r.RemoteAddr); err == nil {
		return host
	}
	return r.RemoteAddr
}

// newToken mints a 256-bit random session token, records it with its role, and
// opportunistically garbage-collects expired tokens.
func (a *Authenticator) newToken(role Role) string {
	var b [32]byte
	_, _ = rand.Read(b[:]) // crypto/rand.Read never returns a short read or error
	tok := hex.EncodeToString(b[:])

	now := time.Now()
	a.mu.Lock()
	for k, s := range a.tokens {
		if now.After(s.exp) {
			delete(a.tokens, k)
		}
	}
	a.tokens[tok] = session{role: role, exp: now.Add(sessionTTL)}
	a.mu.Unlock()
	return tok
}

// sessionRole returns the live session's role, or RoleNone. Auth disabled
// means everyone is effectively vendor (everything open). A 256-bit random
// token makes the map lookup safe without constant-time comparison.
func (a *Authenticator) sessionRole(r *http.Request) Role {
	if !a.Enabled() {
		return RoleVendor
	}
	c, err := r.Cookie(cookieName)
	if err != nil {
		return RoleNone
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	s, ok := a.tokens[c.Value]
	if !ok {
		return RoleNone
	}
	if time.Now().After(s.exp) {
		delete(a.tokens, c.Value)
		return RoleNone
	}
	return s.role
}

func writeJSON(w http.ResponseWriter, status int, body any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(body)
}
