package auth

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// open is a sentinel next-handler that records being reached.
func okHandler() (http.Handler, *bool) {
	reached := new(bool)
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		*reached = true
		w.WriteHeader(http.StatusOK)
	})
	return h, reached
}

func vendorAll(*http.Request) Role { return RoleVendor }

// routeMatrix mirrors main.go's requiredRole: vendor/tuner prefixes, and
// operator on non-GET machine/orders/source.
func routeMatrix(r *http.Request) Role {
	p := r.URL.Path
	switch {
	case strings.HasPrefix(p, "/api/ctdrive/"):
		return RoleVendor
	case strings.HasPrefix(p, "/api/driveshear/"):
		return RoleTuner
	case r.Method != http.MethodGet &&
		(strings.HasPrefix(p, "/api/machine/") ||
			strings.HasPrefix(p, "/api/orders") ||
			strings.HasPrefix(p, "/api/source")):
		return RoleOperator
	default:
		return RoleNone
	}
}

func TestDisabledPassesEverything(t *testing.T) {
	a := New("", "", "", false)
	if a.Enabled() {
		t.Fatal("empty hashes should disable auth")
	}
	next, reached := okHandler()
	rec := httptest.NewRecorder()
	a.Wrap(next, vendorAll).ServeHTTP(rec, httptest.NewRequest("GET", "/api/ctdrive/params", nil))
	if !*reached || rec.Code != http.StatusOK {
		t.Fatalf("disabled auth must pass through; reached=%v code=%d", *reached, rec.Code)
	}
}

func TestProtectedBlockedWithoutSession(t *testing.T) {
	hash, _ := HashPassword("hunter2")
	a := New(hash, "", "", false)
	next, reached := okHandler()
	rec := httptest.NewRecorder()
	a.Wrap(next, vendorAll).ServeHTTP(rec, httptest.NewRequest("GET", "/api/ctdrive/params", nil))
	if *reached {
		t.Fatal("protected route reached without a session")
	}
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("want 401, got %d", rec.Code)
	}
}

func TestUnprotectedAlwaysOpen(t *testing.T) {
	hash, _ := HashPassword("hunter2")
	a := New(hash, "", "", false)
	next, reached := okHandler()
	rec := httptest.NewRecorder()
	a.Wrap(next, routeMatrix).ServeHTTP(rec, httptest.NewRequest("GET", "/api/machine/state", nil))
	if !*reached || rec.Code != http.StatusOK {
		t.Fatalf("unprotected route must pass; reached=%v code=%d", *reached, rec.Code)
	}
}

// TestOperatorTierBackCompat: with no operator hash configured, operator-tier
// writes stay open even though auth (vendor) is enabled — existing two-tier
// deployments must not break.
func TestOperatorTierBackCompat(t *testing.T) {
	vh, _ := HashPassword("vendorpw")
	a := New(vh, "", "", false)
	if a.OperatorGated() {
		t.Fatal("operator tier must be off without a hash")
	}
	next, reached := okHandler()
	rec := httptest.NewRecorder()
	a.Wrap(next, routeMatrix).ServeHTTP(rec, httptest.NewRequest("POST", "/api/machine/produce", nil))
	if !*reached || rec.Code != http.StatusOK {
		t.Fatalf("operator write must stay open without operator hash; reached=%v code=%d", *reached, rec.Code)
	}
}

// TestOperatorTier: with an operator hash set, operator-surface writes need a
// session (any role), reads stay open, and the operator role does not unlock
// tuner/vendor surfaces.
func TestOperatorTier(t *testing.T) {
	vh, _ := HashPassword("vendorpw")
	oh, _ := HashPassword("operatorpw")
	a := New(vh, "", oh, false)

	try := func(cookie *http.Cookie, method, path string) int {
		next, _ := okHandler()
		rec := httptest.NewRecorder()
		req := httptest.NewRequest(method, path, nil)
		if cookie != nil {
			req.AddCookie(cookie)
		}
		a.Wrap(next, routeMatrix).ServeHTTP(rec, req)
		return rec.Code
	}

	// Not logged in: writes blocked, reads open.
	if c := try(nil, "POST", "/api/machine/produce"); c != http.StatusUnauthorized {
		t.Errorf("anon operator write: want 401, got %d", c)
	}
	if c := try(nil, "POST", "/api/orders/start"); c != http.StatusUnauthorized {
		t.Errorf("anon orders write: want 401, got %d", c)
	}
	if c := try(nil, "GET", "/api/machine/state"); c != http.StatusOK {
		t.Errorf("anon machine read: want 200, got %d", c)
	}
	if c := try(nil, "GET", "/api/orders/progress"); c != http.StatusOK {
		t.Errorf("anon orders read: want 200, got %d", c)
	}

	// Operator password: operator writes open, tuner/vendor surfaces stay shut.
	op := login(t, a, "operatorpw")
	if op == nil {
		t.Fatal("operator password rejected")
	}
	if c := try(op, "POST", "/api/machine/produce"); c != http.StatusOK {
		t.Errorf("operator on machine write: want 200, got %d", c)
	}
	if c := try(op, "POST", "/api/source"); c != http.StatusOK {
		t.Errorf("operator on source write: want 200, got %d", c)
	}
	if c := try(op, "POST", "/api/driveshear/cmd"); c != http.StatusUnauthorized {
		t.Errorf("operator on tuner route: want 401, got %d", c)
	}
	if c := try(op, "GET", "/api/ctdrive/params"); c != http.StatusUnauthorized {
		t.Errorf("operator on vendor route: want 401, got %d", c)
	}

	// Vendor satisfies operator.
	vendor := login(t, a, "vendorpw")
	if vendor == nil {
		t.Fatal("vendor password rejected")
	}
	if c := try(vendor, "POST", "/api/machine/produce"); c != http.StatusOK {
		t.Errorf("vendor on operator write: want 200, got %d", c)
	}
}

// login posts the password and returns the session cookie (nil on failure).
func login(t *testing.T, a *Authenticator, pw string) *http.Cookie {
	t.Helper()
	rec := httptest.NewRecorder()
	a.handleLogin(rec, httptest.NewRequest("POST", "/api/login", strings.NewReader(`{"password":"`+pw+`"}`)))
	for _, c := range rec.Result().Cookies() {
		if c.Name == cookieName && c.Value != "" {
			return c
		}
	}
	return nil
}

func TestLoginGrantsAccess(t *testing.T) {
	hash, _ := HashPassword("hunter2")
	a := New(hash, "", "", false)

	// Wrong password -> 401, no cookie.
	rec := httptest.NewRecorder()
	a.handleLogin(rec, httptest.NewRequest("POST", "/api/login", strings.NewReader(`{"password":"nope"}`)))
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("wrong password: want 401, got %d", rec.Code)
	}
	if len(rec.Result().Cookies()) != 0 {
		t.Fatal("wrong password must not set a cookie")
	}

	// Right password -> session cookie that unlocks a protected route.
	cookie := login(t, a, "hunter2")
	if cookie == nil {
		t.Fatal("login did not set a session cookie")
	}
	if !cookie.HttpOnly {
		t.Error("session cookie must be HttpOnly")
	}
	next, reached := okHandler()
	rec = httptest.NewRecorder()
	req := httptest.NewRequest("GET", "/api/ctdrive/params", nil)
	req.AddCookie(cookie)
	a.Wrap(next, vendorAll).ServeHTTP(rec, req)
	if !*reached || rec.Code != http.StatusOK {
		t.Fatalf("valid session must pass; reached=%v code=%d", *reached, rec.Code)
	}

	// Logout invalidates it.
	logoutReq := httptest.NewRequest("POST", "/api/logout", nil)
	logoutReq.AddCookie(cookie)
	a.handleLogout(httptest.NewRecorder(), logoutReq)

	next2, reached2 := okHandler()
	rec = httptest.NewRecorder()
	req = httptest.NewRequest("GET", "/api/ctdrive/params", nil)
	req.AddCookie(cookie)
	a.Wrap(next2, vendorAll).ServeHTTP(rec, req)
	if *reached2 || rec.Code != http.StatusUnauthorized {
		t.Fatalf("logged-out cookie must be rejected; reached=%v code=%d", *reached2, rec.Code)
	}
}

// TestTunerRoleMatrix: the tuner password opens tuner routes but not vendor
// routes; the vendor password opens both (意見稿 §8.3).
func TestTunerRoleMatrix(t *testing.T) {
	vh, _ := HashPassword("vendorpw")
	th, _ := HashPassword("tunerpw")
	a := New(vh, th, "", false)

	try := func(cookie *http.Cookie, path string) int {
		next, _ := okHandler()
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("GET", path, nil)
		if cookie != nil {
			req.AddCookie(cookie)
		}
		a.Wrap(next, routeMatrix).ServeHTTP(rec, req)
		return rec.Code
	}

	tuner := login(t, a, "tunerpw")
	if tuner == nil {
		t.Fatal("tuner password rejected")
	}
	if c := try(tuner, "/api/driveshear/cmd"); c != http.StatusOK {
		t.Errorf("tuner on tuner route: want 200, got %d", c)
	}
	if c := try(tuner, "/api/ctdrive/params"); c != http.StatusUnauthorized {
		t.Errorf("tuner on vendor route: want 401, got %d", c)
	}

	vendor := login(t, a, "vendorpw")
	if vendor == nil {
		t.Fatal("vendor password rejected")
	}
	if c := try(vendor, "/api/driveshear/cmd"); c != http.StatusOK {
		t.Errorf("vendor on tuner route: want 200, got %d", c)
	}
	if c := try(vendor, "/api/ctdrive/params"); c != http.StatusOK {
		t.Errorf("vendor on vendor route: want 200, got %d", c)
	}
}

// TestLoggedIn: the write gate used by the WebSocket command path. With auth
// disabled every request may write; with auth enabled only a request carrying
// a valid session of any role may.
func TestLoggedIn(t *testing.T) {
	// Disabled: everyone may write.
	if !New("", "", "", false).LoggedIn(httptest.NewRequest("GET", "/ws", nil)) {
		t.Fatal("auth disabled: LoggedIn must be true")
	}

	hash, _ := HashPassword("hunter2")
	a := New(hash, "", "", false)

	// No cookie: blocked.
	if a.LoggedIn(httptest.NewRequest("GET", "/ws", nil)) {
		t.Fatal("no session: LoggedIn must be false")
	}

	// Valid session: allowed.
	cookie := login(t, a, "hunter2")
	if cookie == nil {
		t.Fatal("login did not set a session cookie")
	}
	req := httptest.NewRequest("GET", "/ws", nil)
	req.AddCookie(cookie)
	if !a.LoggedIn(req) {
		t.Fatal("valid session: LoggedIn must be true")
	}
}

func TestSecureFlagFollowsTLS(t *testing.T) {
	hash, _ := HashPassword("x")
	a := New(hash, "", "", true) // serving TLS
	rec := httptest.NewRecorder()
	a.handleLogin(rec, httptest.NewRequest("POST", "/api/login", strings.NewReader(`{"password":"x"}`)))
	for _, c := range rec.Result().Cookies() {
		if c.Name == cookieName && !c.Secure {
			t.Fatal("cookie must be Secure when serving TLS")
		}
	}
}
