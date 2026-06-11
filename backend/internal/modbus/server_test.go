package modbus

import (
	"net"
	"testing"
)

func TestParseAllowlist(t *testing.T) {
	// Empty input => allow-all (nil list).
	if al, err := ParseAllowlist("  "); err != nil || al != nil {
		t.Fatalf("empty input: want (nil,nil), got (%v,%v)", al, err)
	}

	al, err := ParseAllowlist("10.0.0.5, 192.168.1.0/24")
	if err != nil {
		t.Fatalf("parse: %v", err)
	}
	cases := []struct {
		ip   string
		want bool
	}{
		{"10.0.0.5", true},    // exact host
		{"10.0.0.6", false},   // host route does not widen
		{"192.168.1.42", true},// inside CIDR
		{"192.168.2.42", false},
	}
	for _, c := range cases {
		if got := al.allows(net.ParseIP(c.ip)); got != c.want {
			t.Errorf("allows(%s) = %v, want %v", c.ip, got, c.want)
		}
	}

	if _, err := ParseAllowlist("not-an-ip"); err == nil {
		t.Error("invalid token must error")
	}
}

func TestAllowlistEmptyAllowsAll(t *testing.T) {
	var al Allowlist
	if !al.allows(net.ParseIP("203.0.113.1")) {
		t.Error("nil allowlist must allow everyone")
	}
}
