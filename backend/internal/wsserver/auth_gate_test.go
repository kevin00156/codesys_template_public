package wsserver

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
	"codesys_dev/backend/internal/auth"
	"codesys_dev/backend/internal/state"
)

func TestCommandRole(t *testing.T) {
	cases := map[string]auth.Role{
		"machine":    auth.RoleOperator,
		"production": auth.RoleOperator,
		"axis":       auth.RoleTuner,
		"bogus":      auth.RoleNone,
	}
	for typ, want := range cases {
		if got := commandRole(typ); got != want {
			t.Errorf("commandRole(%q) = %q, want %q", typ, got, want)
		}
	}
}

// TestCommandPlaneGated stands up a real WebSocket and proves the login wall
// reaches the command plane: an anonymous socket may watch but not command,
// while an authenticated vendor session commands successfully.
func TestCommandPlaneGated(t *testing.T) {
	// Operator hash configured so the operator-tier "machine" command is gated
	// (with vendor only, operator surfaces stay open by back-compat design).
	vendorHash, _ := auth.HashPassword("vendorpw")
	operatorHash, _ := auth.HashPassword("operatorpw")
	authn := auth.New(vendorHash, "", operatorHash, false)

	srv := &Server{
		Snapshot: &state.Snapshot{},
		Commands: &fakeSink{},
		Auth:     authn,
		Interval: time.Hour, // suppress telemetry pushes; we only test acks
	}
	mux := http.NewServeMux()
	authn.RegisterRoutes(mux)
	mux.Handle("/ws", srv)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	wsURL := "ws" + strings.TrimPrefix(ts.URL, "http") + "/ws"

	// Anonymous: command is refused.
	if ack := sendCmd(t, wsURL, nil); ack.OK {
		t.Fatalf("anonymous machine command should be refused, got ok=true")
	} else if ack.Error == "" {
		t.Fatal("refusal must carry an error message")
	}

	// Log in, capture the session cookie, command with it.
	resp, err := http.Post(ts.URL+"/api/login", "application/json", strings.NewReader(`{"password":"vendorpw"}`))
	if err != nil {
		t.Fatalf("login: %v", err)
	}
	resp.Body.Close()
	cookies := resp.Cookies()
	if len(cookies) == 0 {
		t.Fatal("login returned no cookie")
	}
	hdr := http.Header{}
	for _, c := range cookies {
		hdr.Add("Cookie", c.String())
	}
	if ack := sendCmd(t, wsURL, hdr); !ack.OK {
		t.Fatalf("authenticated command should succeed, got error %q", ack.Error)
	}
}

// sendCmd dials the socket (optionally with auth headers), sends one machine
// command, and returns the first ack.
func sendCmd(t *testing.T, url string, hdr http.Header) AckMsg {
	t.Helper()
	conn, _, err := websocket.DefaultDialer.Dial(url, hdr)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer conn.Close()
	if err := conn.WriteJSON(CmdMsg{Type: "machine"}); err != nil {
		t.Fatalf("write: %v", err)
	}
	conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	var ack AckMsg
	if err := conn.ReadJSON(&ack); err != nil {
		t.Fatalf("read ack: %v", err)
	}
	return ack
}
