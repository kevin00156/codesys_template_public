// Package wsserver serves a WebSocket endpoint that pushes PlcData
// snapshots to all connected clients and accepts command messages.
package wsserver

import (
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"time"

	"github.com/gorilla/websocket"
	"codesys_dev/backend/internal/auth"
	"codesys_dev/backend/internal/shm"
	"codesys_dev/backend/internal/state"
)

// CommandSink is the same interface used by the Modbus server.
type CommandSink interface {
	Apply(func(*shm.PlcCommand) error) error
}

// Authorizer gates the command plane. *auth.Authenticator satisfies it. When
// nil, every command is allowed — the dev posture, and what the unit tests use.
type Authorizer interface {
	Allows(r *http.Request, required auth.Role) bool
	SessionRole(r *http.Request) auth.Role
}

type Server struct {
	Snapshot *state.Snapshot
	Commands CommandSink
	Auth     Authorizer    // nil => command plane is open (auth disabled / dev)
	Interval time.Duration // push interval; defaults to 100ms
}

// commandRole is the minimum role required to issue a command of a given type.
// This is the command-plane equivalent of main.go's requiredRole predicate and
// the real enforcement point: telemetry pushes stay open (the dashboard is
// always-on), but a write to the machine must clear this bar.
//
//   - machine / production: routine operator surfaces (start, stop, run an order)
//   - axis:                 jog / absolute move — a tuning / debug surface
//
// Tighten or relax per deployment; vendor ⊇ tuner ⊇ operator.
func commandRole(t string) auth.Role {
	switch t {
	case "machine", "production":
		return auth.RoleOperator
	case "axis":
		return auth.RoleTuner
	default:
		return auth.RoleNone // unknown — applyCmd rejects it anyway
	}
}

// No CheckOrigin override: gorilla's default rejects cross-origin upgrades
// (Origin host must match the request Host; non-browser clients without an
// Origin header pass). This is the wall against cross-site WebSocket
// hijacking — a page on another site can't open our telemetry/command socket
// with the operator's ambient session.
var upgrader = websocket.Upgrader{}

func (s *Server) interval() time.Duration {
	if s.Interval > 0 {
		return s.Interval
	}
	return 100 * time.Millisecond
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		log.Printf("ws upgrade: %v", err)
		return
	}
	defer conn.Close()

	// gorilla/websocket forbids concurrent writers. All writes — periodic
	// data pushes and command acks — go through this single goroutine.
	acks := make(chan AckMsg, 4)
	done := make(chan struct{})

	go func() {
		tick := time.NewTicker(s.interval())
		defer tick.Stop()
		for {
			select {
			case <-done:
				return
			case <-tick.C:
				d, _, ok := s.Snapshot.Read()
				if !ok {
					continue
				}
				if err := conn.WriteJSON(dataFromPlc(&d)); err != nil {
					conn.Close() // unblock the reader so it tears down
					return
				}
			case ack := <-acks:
				if err := conn.WriteJSON(ack); err != nil {
					conn.Close()
					return
				}
			}
		}
	}()

	for {
		_, raw, err := conn.ReadMessage()
		if err != nil {
			break
		}
		ack := AckMsg{Type: "ack", OK: true}
		var cmd CmdMsg
		if err := json.Unmarshal(raw, &cmd); err != nil {
			ack.OK, ack.Error = false, "invalid json"
		} else if s.Auth != nil && !s.Auth.Allows(r, commandRole(cmd.Type)) {
			// The login wall, enforced on the control path: an unauthenticated
			// (or under-privileged) socket may watch telemetry but not command.
			ack.OK, ack.Error = false, "需要登入"
		} else if applyErr := s.applyCmd(&cmd); applyErr != nil {
			ack.OK, ack.Error = false, applyErr.Error()
		}
		s.auditCmd(r, &cmd, ack)
		// Non-blocking: if the writer is gone (or backed up) we drop the
		// ack rather than deadlock; the next ReadMessage will see the
		// closed conn and break.
		select {
		case acks <- ack:
		default:
		}
	}
	close(done)
}

func (s *Server) applyCmd(cmd *CmdMsg) error {
	if s.Commands == nil {
		return fmt.Errorf("command sink unavailable (PLC shm not mounted)")
	}
	return s.Commands.Apply(func(c *shm.PlcCommand) error {
		switch cmd.Type {
		case "machine":
			c.Machine.ControlFlags = cmd.ControlFlags
		case "axis":
			if cmd.AxisIndex < 0 || cmd.AxisIndex >= len(c.Machine.Axes) {
				return fmt.Errorf("axis index %d out of range", cmd.AxisIndex)
			}
			a := &c.Machine.Axes[cmd.AxisIndex]
			a.ControlFlags = cmd.AxisFlags
			a.JogVel       = cmd.JogVel
			a.MoveAbsPos   = cmd.MoveAbsPos
			a.MoveAbsVel   = cmd.MoveAbsVel
		case "production":
			c.Production.NProductionState = cmd.NProductionState
		default:
			return fmt.Errorf("unknown command type %q", cmd.Type)
		}
		return nil
	})
}

// auditCmd logs every command-plane attempt — who (client addr + role), what
// (type), and the outcome. An industrial control surface needs an attributable
// trail of who moved the machine; this is that trail.
func (s *Server) auditCmd(r *http.Request, cmd *CmdMsg, ack AckMsg) {
	role := auth.RoleNone
	if s.Auth != nil {
		role = s.Auth.SessionRole(r)
	}
	if role == auth.RoleNone {
		role = "-"
	}
	if ack.OK {
		log.Printf("ws cmd from=%s role=%s type=%s ok", r.RemoteAddr, role, cmd.Type)
		return
	}
	log.Printf("ws cmd from=%s role=%s type=%s denied=%q", r.RemoteAddr, role, cmd.Type, ack.Error)
}
