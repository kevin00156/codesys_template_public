// Package cmdsink serialises command writes from every source (WebSocket,
// Modbus) and publishes the latest full command struct to /dev/shm/plc_cmd.
//
// It also owns the jog dead-man: the command word is level-held (the PLC acts
// on whatever is set each cycle), so a jog bit whose writer vanished — browser
// crash, WebSocket drop, Modbus master gone — would keep the axis moving
// forever. Clients must re-send the jog command periodically; the watchdog
// clears any axis's jog bits that stop being refreshed.
//
// Every publish also stamps Header.Flags with the axes it is about (see
// shm.CmdFlagsAxisMaskPresent): a reader that latches on Header.Cycle must not
// mistake an unrelated publish for a new request on an axis whose one-shot bit
// (MoveAbs, Home) is still parked in the level-held word.
package cmdsink

import (
	"context"
	"log"
	"sync"
	"time"

	"codesys_dev/backend/internal/shm"
)

// Sink applies mutations to a pending PlcCommand and publishes it. Atomic-ish
// semantics: a closure that returns non-nil error rolls back any field
// mutations it made, so a rejected partial write never reaches the PLC.
type Sink struct {
	mu      sync.Mutex
	publish func(*shm.PlcCommand)
	pending shm.PlcCommand

	// jogSeen[i] is the last time Apply left axis i with a jog bit set —
	// i.e. the jog was commanded or refreshed. The watchdog clears jog bits
	// older than its timeout.
	jogSeen [4]time.Time
}

// New returns a Sink that hands each published command to publish
// (production: a closure around shm.WritePlcCommand).
func New(publish func(*shm.PlcCommand)) *Sink {
	s := &Sink{publish: publish}
	s.pending.Header.Magic = shm.PlcCommandMagic
	s.pending.Header.Version = shm.PlcCommandVersion
	return s
}

// SeedCycle continues Header.Cycle from c — the value found in the segment
// when the bridge started — so the first publish uses c+1.
//
// Readers detect a new command by Cycle changing and may have latched the
// previous bridge's last value. A sink that restarted at 1 would replay
// numbers they have already seen, so the first (all-clear) command after a
// crash could be ignored as "not new" and the machine would keep executing
// the last word — possibly a jog. Call before the first Apply.
func (s *Sink) SeedCycle(c uint64) *Sink {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.pending.Header.Cycle = c
	return s
}

// Apply runs fn against the pending command under the sink's mutex and
// publishes the result. A non-nil error from fn rolls everything back.
//
// The publish names the axes whose command fn changed (struct diff). That is
// the right default for a source that just writes whatever it was handed —
// the Modbus path funnels every register write through here and has no
// notion of "which axis this message is for". A source that does know should
// use ApplyTouching.
func (s *Sink) Apply(fn func(*shm.PlcCommand) error) error {
	return s.ApplyTouching(0, fn)
}

// ApplyTouching is Apply with an explicit axis mask (built from
// shm.CmdFlagsAxisTouched) ORed into the diff-derived one. Pass the axis a
// message addresses even when the resulting struct is unchanged: an operator
// re-sending an identical word (the same MoveAbs target twice) means "do it
// again", which a diff alone cannot tell from a no-op. A mask of 0 with an
// unchanged struct publishes with no axis touched — the marker bit is always
// set, so readers know that means "nobody", not "unknown".
func (s *Sink) ApplyTouching(mask uint16, fn func(*shm.PlcCommand) error) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	saved := s.pending
	if err := fn(&s.pending); err != nil {
		s.pending = saved
		return err
	}
	touched := mask
	now := time.Now()
	for i := range s.pending.Machine.Axes {
		if s.pending.Machine.Axes[i] != saved.Machine.Axes[i] {
			touched |= shm.CmdFlagsAxisTouched(i)
		}
		if s.pending.Machine.Axes[i].ControlFlags&(shm.AxisCtrlJogPos|shm.AxisCtrlJogNeg) != 0 {
			s.jogSeen[i] = now
		}
	}
	s.publishLocked(touched)
	return nil
}

// publishLocked stamps the touched mask and the next cycle number, then
// hands the pending command to the publisher. Caller holds s.mu.
func (s *Sink) publishLocked(touched uint16) {
	s.pending.Header.Flags = shm.CmdFlagsAxisMaskPresent | touched
	s.pending.Header.Cycle++
	s.publish(&s.pending)
}

// StartJogWatchdog clears any axis's jog bits that have not been refreshed
// (via Apply) within timeout, and republishes. Runs until ctx is cancelled.
func (s *Sink) StartJogWatchdog(ctx context.Context, timeout time.Duration) {
	go func() {
		tick := time.NewTicker(timeout / 4)
		defer tick.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-tick.C:
				s.expireJogs(time.Now(), timeout)
			}
		}
	}()
}

func (s *Sink) expireJogs(now time.Time, timeout time.Duration) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var cleared uint16 // axes whose jog bits we dropped — the only ones this publish is about
	for i := range s.pending.Machine.Axes {
		a := &s.pending.Machine.Axes[i]
		if a.ControlFlags&(shm.AxisCtrlJogPos|shm.AxisCtrlJogNeg) == 0 {
			continue
		}
		if now.Sub(s.jogSeen[i]) < timeout {
			continue
		}
		a.ControlFlags &^= shm.AxisCtrlJogPos | shm.AxisCtrlJogNeg
		cleared |= shm.CmdFlagsAxisTouched(i)
		log.Printf("cmdsink: jog watchdog cleared axis %d (no refresh for %s)", i, timeout)
	}
	if cleared != 0 {
		s.publishLocked(cleared)
	}
}
