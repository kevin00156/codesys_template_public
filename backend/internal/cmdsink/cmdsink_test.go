package cmdsink

import (
	"errors"
	"testing"
	"time"

	"codesys_dev/backend/internal/shm"
)

// capture records every published command.
type capture struct {
	last  shm.PlcCommand
	count int
}

func (c *capture) publish(cmd *shm.PlcCommand) {
	c.last = *cmd
	c.count++
}

// wantFlags asserts the published Header.Flags carries the marker plus
// exactly the given axis bits.
func wantFlags(t *testing.T, got, axes uint16) {
	t.Helper()
	if got&shm.CmdFlagsAxisMaskPresent == 0 {
		t.Errorf("Header.Flags %#x: axis-mask marker not set", got)
	}
	if want := shm.CmdFlagsAxisMaskPresent | axes; got != want {
		t.Errorf("Header.Flags: got %#x want %#x", got, want)
	}
}

func TestApply_PublishesAndCounts(t *testing.T) {
	var pub capture
	s := New(pub.publish)

	if err := s.Apply(func(c *shm.PlcCommand) error {
		c.Machine.ControlFlags = 4
		return nil
	}); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	if pub.count != 1 || pub.last.Machine.ControlFlags != 4 {
		t.Fatalf("publish: count=%d flags=%d", pub.count, pub.last.Machine.ControlFlags)
	}
	if pub.last.Header.Magic != shm.PlcCommandMagic || pub.last.Header.Version != shm.PlcCommandVersion {
		t.Errorf("header not initialised: %+v", pub.last.Header)
	}
	if pub.last.Header.Cycle != 1 {
		t.Errorf("cycle: got %d want 1", pub.last.Header.Cycle)
	}
	// Machine-only change: marker set, no axis touched.
	wantFlags(t, pub.last.Header.Flags, 0)
}

func TestApply_RollsBackOnError(t *testing.T) {
	var pub capture
	s := New(pub.publish)

	if err := s.Apply(func(c *shm.PlcCommand) error {
		c.Machine.ControlFlags = 99 // partial mutation that must not survive
		return errors.New("rejected")
	}); err == nil {
		t.Fatal("want error from Apply")
	}
	if err := s.ApplyTouching(shm.CmdFlagsAxisTouched(0), func(c *shm.PlcCommand) error {
		c.Machine.Axes[0].MoveAbsPos = 7
		return errors.New("rejected")
	}); err == nil {
		t.Fatal("want error from ApplyTouching")
	}
	if pub.count != 0 {
		t.Fatalf("rejected write was published (count=%d)", pub.count)
	}

	// The pending state must be clean for the next writer.
	if err := s.Apply(func(c *shm.PlcCommand) error { return nil }); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	if pub.last.Machine.ControlFlags != 0 || pub.last.Machine.Axes[0].MoveAbsPos != 0 {
		t.Errorf("rolled-back mutation leaked: %+v", pub.last.Machine)
	}
	// Nothing changed and nobody was named: no axis re-armed.
	wantFlags(t, pub.last.Header.Flags, 0)
}

func TestApply_TouchedMaskFollowsDiff(t *testing.T) {
	var pub capture
	s := New(pub.publish)

	// Axis 2 gets a MoveAbs: only its bit is set.
	if err := s.Apply(func(c *shm.PlcCommand) error {
		c.Machine.Axes[2].ControlFlags = 1 << 6
		c.Machine.Axes[2].MoveAbsPos = 50
		return nil
	}); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	wantFlags(t, pub.last.Header.Flags, shm.CmdFlagsAxisTouched(2))

	// The defect this guards against: axis 2's MoveAbs bit is still parked in
	// the level-held word, but a machine-only publish must not name axis 2,
	// or a cycle-latching reader would re-dispatch the finished move.
	if err := s.Apply(func(c *shm.PlcCommand) error {
		c.Machine.ControlFlags = 1
		return nil
	}); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	if pub.last.Machine.Axes[2].ControlFlags != 1<<6 {
		t.Fatalf("axis 2 word should still hold MoveAbs: %#x", pub.last.Machine.Axes[2].ControlFlags)
	}
	wantFlags(t, pub.last.Header.Flags, 0)

	// Two axes changed in one closure: both bits.
	if err := s.Apply(func(c *shm.PlcCommand) error {
		c.Machine.Axes[0].JogVel = 1
		c.Machine.Axes[3].JogVel = 1
		return nil
	}); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	wantFlags(t, pub.last.Header.Flags, shm.CmdFlagsAxisTouched(0)|shm.CmdFlagsAxisTouched(3))
}

func TestApplyTouching_ExplicitMaskWithoutChange(t *testing.T) {
	var pub capture
	s := New(pub.publish)

	move := func(c *shm.PlcCommand) error {
		c.Machine.Axes[1].ControlFlags = 1 << 6
		c.Machine.Axes[1].MoveAbsPos = 100
		c.Machine.Axes[1].MoveAbsVel = 10
		return nil
	}
	if err := s.ApplyTouching(shm.CmdFlagsAxisTouched(1), move); err != nil {
		t.Fatalf("ApplyTouching: %v", err)
	}
	wantFlags(t, pub.last.Header.Flags, shm.CmdFlagsAxisTouched(1))

	// Operator sends the identical word again: the struct does not change, so
	// a diff sees nothing — the explicit mask is what makes it a fresh request.
	if err := s.ApplyTouching(shm.CmdFlagsAxisTouched(1), move); err != nil {
		t.Fatalf("ApplyTouching: %v", err)
	}
	if pub.count != 2 {
		t.Fatalf("publish count: got %d want 2", pub.count)
	}
	wantFlags(t, pub.last.Header.Flags, shm.CmdFlagsAxisTouched(1))

	// Explicit mask and diff are ORed together.
	if err := s.ApplyTouching(shm.CmdFlagsAxisTouched(1), func(c *shm.PlcCommand) error {
		c.Machine.Axes[3].ControlFlags = 1
		return nil
	}); err != nil {
		t.Fatalf("ApplyTouching: %v", err)
	}
	wantFlags(t, pub.last.Header.Flags, shm.CmdFlagsAxisTouched(1)|shm.CmdFlagsAxisTouched(3))
}

func TestSeedCycle_ContinuesFromSegment(t *testing.T) {
	var pub capture
	// The bridge found cycle 41 in the segment left by its predecessor.
	s := New(pub.publish).SeedCycle(41)

	if err := s.Apply(func(c *shm.PlcCommand) error { return nil }); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	if pub.last.Header.Cycle != 42 {
		t.Errorf("first publish after seed: cycle %d want 42", pub.last.Header.Cycle)
	}
	if err := s.Apply(func(c *shm.PlcCommand) error { return nil }); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	if pub.last.Header.Cycle != 43 {
		t.Errorf("second publish after seed: cycle %d want 43", pub.last.Header.Cycle)
	}
	if pub.last.Header.Magic != shm.PlcCommandMagic || pub.last.Header.Version != shm.PlcCommandVersion {
		t.Errorf("seeding disturbed the header: %+v", pub.last.Header)
	}
}

func TestJogWatchdog_ClearsUnrefreshedJog(t *testing.T) {
	var pub capture
	s := New(pub.publish)

	if err := s.Apply(func(c *shm.PlcCommand) error {
		c.Machine.Axes[1].ControlFlags = shm.AxisCtrlJogPos
		c.Machine.Axes[1].JogVel = 5
		return nil
	}); err != nil {
		t.Fatalf("Apply: %v", err)
	}

	const timeout = 20 * time.Millisecond
	// Not yet expired: nothing happens.
	s.expireJogs(time.Now(), timeout)
	if pub.count != 1 {
		t.Fatalf("watchdog fired early (count=%d)", pub.count)
	}

	// Past the timeout: jog bits cleared and republished, naming only the
	// axis whose jog was dropped.
	s.expireJogs(time.Now().Add(2*timeout), timeout)
	if pub.count != 2 {
		t.Fatalf("watchdog did not republish (count=%d)", pub.count)
	}
	if pub.last.Machine.Axes[1].ControlFlags&(shm.AxisCtrlJogPos|shm.AxisCtrlJogNeg) != 0 {
		t.Errorf("jog bits not cleared: %#x", pub.last.Machine.Axes[1].ControlFlags)
	}
	wantFlags(t, pub.last.Header.Flags, shm.CmdFlagsAxisTouched(1))
}

func TestJogWatchdog_RefreshKeepsJogAlive(t *testing.T) {
	var pub capture
	s := New(pub.publish)

	jog := func() {
		if err := s.Apply(func(c *shm.PlcCommand) error {
			c.Machine.Axes[0].ControlFlags = shm.AxisCtrlJogNeg
			return nil
		}); err != nil {
			t.Fatalf("Apply: %v", err)
		}
	}

	const timeout = 50 * time.Millisecond
	jog()
	time.Sleep(timeout / 2)
	jog() // client keepalive re-send
	s.expireJogs(time.Now(), timeout)
	if pub.last.Machine.Axes[0].ControlFlags&shm.AxisCtrlJogNeg == 0 {
		t.Error("refreshed jog was cleared")
	}

	// Non-jog commands on another axis must not refresh axis 0's jog.
	time.Sleep(timeout + timeout/2)
	s.expireJogs(time.Now(), timeout)
	if pub.last.Machine.Axes[0].ControlFlags&shm.AxisCtrlJogNeg != 0 {
		t.Error("expired jog survived")
	}
}
