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
	"codesys_dev/backend/internal/shm"
	"codesys_dev/backend/internal/state"
)

// CommandSink is the same interface used by the Modbus server.
type CommandSink interface {
	Apply(func(*shm.PlcCommand) error) error
}

type Server struct {
	Snapshot *state.Snapshot
	Commands CommandSink
	Interval time.Duration // push interval; defaults to 100ms

	// AuthorizeWrite, if set, gates command (write) messages. It is evaluated
	// once per connection against the upgrade request — which carries the
	// session cookie — so an unauthenticated socket can still receive the live
	// data push but every command it sends is rejected with an "unauthorized"
	// ack. nil => every connection may write (auth-disabled / dev posture).
	AuthorizeWrite func(r *http.Request) bool
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
	// Decide write permission from the session before the upgrade hijacks the
	// request. Reads (the periodic data push) are never gated.
	canWrite := s.AuthorizeWrite == nil || s.AuthorizeWrite(r)

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
		if !canWrite {
			ack.OK, ack.Error = false, "unauthorized"
		} else if err := json.Unmarshal(raw, &cmd); err != nil {
			ack.OK, ack.Error = false, "invalid json"
		} else if applyErr := s.applyCmd(&cmd); applyErr != nil {
			ack.OK, ack.Error = false, applyErr.Error()
		}
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
			if cmd.AxisIndex >= 0 && cmd.AxisIndex < 4 {
				a := &c.Machine.Axes[cmd.AxisIndex]
				a.ControlFlags = cmd.AxisFlags
				a.JogVel       = cmd.JogVel
				a.MoveAbsPos   = cmd.MoveAbsPos
				a.MoveAbsVel   = cmd.MoveAbsVel
			}
		case "production":
			c.Production.NProductionState = cmd.NProductionState
		default:
			return fmt.Errorf("unknown command type %q", cmd.Type)
		}
		return nil
	})
}
