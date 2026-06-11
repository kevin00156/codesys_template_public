package wsserver

import (
	"testing"

	"codesys_dev/backend/internal/shm"
)

// fakeSink applies the closure against an in-memory command struct so we can
// exercise applyCmd without a real shm mapping.
type fakeSink struct{ cmd shm.PlcCommand }

func (f *fakeSink) Apply(fn func(*shm.PlcCommand) error) error { return fn(&f.cmd) }

func TestApplyCmd(t *testing.T) {
	s := &Server{Commands: &fakeSink{}}

	if err := s.applyCmd(&CmdMsg{Type: "machine"}); err != nil {
		t.Errorf("machine command: unexpected error: %v", err)
	}
	if err := s.applyCmd(&CmdMsg{Type: "axis", AxisIndex: 0}); err != nil {
		t.Errorf("axis command: unexpected error: %v", err)
	}
	if err := s.applyCmd(&CmdMsg{Type: "production"}); err != nil {
		t.Errorf("production command: unexpected error: %v", err)
	}
	if err := s.applyCmd(&CmdMsg{Type: "bogus"}); err == nil {
		t.Error("unknown command type: expected an error, got nil")
	}
}

func TestApplyCmd_NilSink(t *testing.T) {
	// A bridge running without shm mounted has a nil CommandSink; commands
	// must error rather than panic.
	s := &Server{}
	if err := s.applyCmd(&CmdMsg{Type: "machine"}); err == nil {
		t.Error("nil command sink: expected an error, got nil")
	}
}
