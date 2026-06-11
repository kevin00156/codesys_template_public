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
	"net/http"
	"sync"
	"time"

	"golang.org/x/crypto/bcrypt"
)

const (
	cookieName = "plc_session"
	sessionTTL = 12 * time.Hour
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

// satisfies reports whether a session role meets a route requirement.
func (r Role) satisfies(required Role) bool {
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
}

// New builds an Authenticator. Empty hashes disable auth entirely: every
// route is open and /api/auth/status reports enabled:false — the intended
// posture on a dev box. secure should be true when serving TLS so the cookie
// is never sent in clear.
func New(vendorHash, tunerHash, operatorHash string, secure bool) *Authenticator {
	a := &Authenticator{secure: secure, tokens: map[string]session{}}
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
			if req != RoleNone && !a.sessionRole(r).satisfies(req) {
				writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "需要登入"})
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

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
