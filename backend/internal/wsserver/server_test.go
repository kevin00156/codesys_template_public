package wsserver

import (
	"testing"

	"codesys_dev/backend/internal/shm"
)

// fakeSink applies the closure against an in-memory command struct so we can
// exercise applyCmd without a real shm mapping. It records the explicit axis
// mask of every call (plain Apply counts as mask 0) so tests can check which
// path a message took.
type fakeSink struct {
	cmd   shm.PlcCommand
	masks []uint16
}

func (f *fakeSink) Apply(fn func(*shm.PlcCommand) error) error { return f.ApplyTouching(0, fn) }

func (f *fakeSink) ApplyTouching(mask uint16, fn func(*shm.PlcCommand) error) error {
	f.masks = append(f.masks, mask)
	return fn(&f.cmd)
}

func TestApplyCmd(t *testing.T) {
	sink := &fakeSink{}
	s := &Server{Commands: sink}

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
	if len(sink.masks) != 3 {
		t.Fatalf("sink calls: got %d want 3 (unknown type must not reach the sink)", len(sink.masks))
	}
}

func TestApplyCmd_AxisNamesItsAxis(t *testing.T) {
	sink := &fakeSink{}
	s := &Server{Commands: sink}

	// Same word twice: the struct does not change on the re-send, so the
	// explicit mask is the only thing that marks it as a fresh request.
	move := &CmdMsg{Type: "axis", AxisIndex: 2, AxisFlags: 1 << 6, MoveAbsPos: 50, MoveAbsVel: 10}
	for i := 0; i < 2; i++ {
		if err := s.applyCmd(move); err != nil {
			t.Fatalf("axis command: %v", err)
		}
	}
	if len(sink.masks) != 2 {
		t.Fatalf("sink calls: got %d want 2", len(sink.masks))
	}
	for i, m := range sink.masks {
		if want := shm.CmdFlagsAxisTouched(2); m != want {
			t.Errorf("call %d: mask %#x want %#x", i, m, want)
		}
	}
	if a := sink.cmd.Machine.Axes[2]; a.ControlFlags != 1<<6 || a.MoveAbsPos != 50 || a.MoveAbsVel != 10 {
		t.Errorf("axis 2 word not written: %+v", a)
	}
}

func TestApplyCmd_MachineAndProductionNameNoAxis(t *testing.T) {
	sink := &fakeSink{}
	s := &Server{Commands: sink}

	if err := s.applyCmd(&CmdMsg{Type: "machine", ControlFlags: 1}); err != nil {
		t.Fatalf("machine command: %v", err)
	}
	if err := s.applyCmd(&CmdMsg{Type: "production", NProductionState: 3}); err != nil {
		t.Fatalf("production command: %v", err)
	}
	for i, m := range sink.masks {
		if m != 0 {
			t.Errorf("call %d: non-axis message named axes %#x", i, m)
		}
	}
}

func TestApplyCmd_AxisIndexOutOfRange(t *testing.T) {
	sink := &fakeSink{}
	s := &Server{Commands: sink}

	for _, idx := range []int{-1, 4, 99} {
		if err := s.applyCmd(&CmdMsg{Type: "axis", AxisIndex: idx}); err == nil {
			t.Errorf("axis index %d: expected an error, got nil", idx)
		}
	}
	if len(sink.masks) != 0 {
		t.Errorf("out-of-range axis reached the sink (%d calls)", len(sink.masks))
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
