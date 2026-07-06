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
	if pub.count != 0 {
		t.Fatalf("rejected write was published (count=%d)", pub.count)
	}

	// The pending state must be clean for the next writer.
	if err := s.Apply(func(c *shm.PlcCommand) error { return nil }); err != nil {
		t.Fatalf("Apply: %v", err)
	}
	if pub.last.Machine.ControlFlags != 0 {
		t.Errorf("rolled-back mutation leaked: flags=%d", pub.last.Machine.ControlFlags)
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

	// Past the timeout: jog bits cleared and republished.
	s.expireJogs(time.Now().Add(2*timeout), timeout)
	if pub.count != 2 {
		t.Fatalf("watchdog did not republish (count=%d)", pub.count)
	}
	if pub.last.Machine.Axes[1].ControlFlags&(shm.AxisCtrlJogPos|shm.AxisCtrlJogNeg) != 0 {
		t.Errorf("jog bits not cleared: %#x", pub.last.Machine.Axes[1].ControlFlags)
	}
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
