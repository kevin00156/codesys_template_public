// Package cmdsink serialises command writes from every source (WebSocket,
// Modbus) and publishes the latest full command struct to /dev/shm/plc_cmd.
//
// It also owns the jog dead-man: the command word is level-held (the PLC acts
// on whatever is set each cycle), so a jog bit whose writer vanished — browser
// crash, WebSocket drop, Modbus master gone — would keep the axis moving
// forever. Clients must re-send the jog command periodically; the watchdog
// clears any axis's jog bits that stop being refreshed.
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

// Apply runs fn against the pending command under the sink's mutex and
// publishes the result. A non-nil error from fn rolls everything back.
func (s *Sink) Apply(fn func(*shm.PlcCommand) error) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	saved := s.pending
	if err := fn(&s.pending); err != nil {
		s.pending = saved
		return err
	}
	now := time.Now()
	for i := range s.pending.Machine.Axes {
		if s.pending.Machine.Axes[i].ControlFlags&(shm.AxisCtrlJogPos|shm.AxisCtrlJogNeg) != 0 {
			s.jogSeen[i] = now
		}
	}
	s.publishLocked()
	return nil
}

func (s *Sink) publishLocked() {
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
	expired := false
	for i := range s.pending.Machine.Axes {
		a := &s.pending.Machine.Axes[i]
		if a.ControlFlags&(shm.AxisCtrlJogPos|shm.AxisCtrlJogNeg) == 0 {
			continue
		}
		if now.Sub(s.jogSeen[i]) < timeout {
			continue
		}
		a.ControlFlags &^= shm.AxisCtrlJogPos | shm.AxisCtrlJogNeg
		expired = true
		log.Printf("cmdsink: jog watchdog cleared axis %d (no refresh for %s)", i, timeout)
	}
	if expired {
		s.publishLocked()
	}
}
