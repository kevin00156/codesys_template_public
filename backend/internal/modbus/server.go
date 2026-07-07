// Modbus TCP slave. Implements function codes 0x03 (read holding),
// 0x06 (write single holding), 0x10 (write multiple holding).
// Anything else returns exception 01 (illegal function).
//
// The implementation is deliberately small and dependency-free —
// Modbus TCP framing is a 7-byte MBAP header plus a PDU, and we only
// need three function codes for plc_bridge's purposes.
package modbus

import (
	"encoding/binary"
	"io"
	"log"
	"net"
	"time"

	"codesys_dev/backend/internal/shm"
	"codesys_dev/backend/internal/state"
)

const (
	fnReadHolding          = 0x03
	fnWriteSingleHolding   = 0x06
	fnWriteMultipleHolding = 0x10

	excIllegalFunction    = 0x01
	excIllegalDataAddress = 0x02
	excIllegalDataValue   = 0x03
	excServerFailure      = 0x04
)

// CommandSink receives validated decoded writes. The implementation
// must serialise concurrent calls and only publish the mutated command
// to /dev/shm/plc_cmd when the closure returns nil. On non-nil error,
// the implementation must roll back any partial mutations.
type CommandSink interface {
	Apply(func(cmd *shm.PlcCommand) error) error
}

type Server struct {
	Snapshot *state.Snapshot
	Commands CommandSink
	// StaleAfter: snapshot age past which reads fail with exception 04
	// (server failure) instead of serving frozen values as live data.
	// Defaults to 500ms.
	StaleAfter time.Duration
}

func (s *Server) staleAfter() time.Duration {
	if s.StaleAfter > 0 {
		return s.StaleAfter
	}
	return 500 * time.Millisecond
}

func (s *Server) ListenAndServe(addr string) error {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return err
	}
	log.Printf("modbus tcp listening on %s", addr)
	for {
		conn, err := ln.Accept()
		if err != nil {
			return err
		}
		go s.handle(conn)
	}
}

func (s *Server) handle(conn net.Conn) {
	defer conn.Close()
	var mbap [7]byte
	for {
		if _, err := io.ReadFull(conn, mbap[:]); err != nil {
			return
		}
		txn := binary.BigEndian.Uint16(mbap[0:2])
		proto := binary.BigEndian.Uint16(mbap[2:4])
		length := binary.BigEndian.Uint16(mbap[4:6])
		unitID := mbap[6]

		// Modbus TCP: length covers UnitID (1 byte) + PDU.
		if proto != 0 || length < 2 || length > 253 {
			return
		}
		pdu := make([]byte, length-1)
		if _, err := io.ReadFull(conn, pdu); err != nil {
			return
		}
		resp := s.dispatch(pdu)
		if len(resp) == 0 {
			return
		}
		out := make([]byte, 7+len(resp))
		binary.BigEndian.PutUint16(out[0:2], txn)
		binary.BigEndian.PutUint16(out[2:4], 0)
		binary.BigEndian.PutUint16(out[4:6], uint16(len(resp)+1))
		out[6] = unitID
		copy(out[7:], resp)
		if _, err := conn.Write(out); err != nil {
			return
		}
	}
}

func (s *Server) dispatch(pdu []byte) []byte {
	if len(pdu) < 1 {
		return nil
	}
	switch pdu[0] {
	case fnReadHolding:
		return s.handleRead(pdu)
	case fnWriteSingleHolding:
		return s.handleWriteSingle(pdu)
	case fnWriteMultipleHolding:
		return s.handleWriteMultiple(pdu)
	default:
		return exception(pdu[0], excIllegalFunction)
	}
}

func (s *Server) handleRead(pdu []byte) []byte {
	if len(pdu) != 5 {
		return exception(pdu[0], excIllegalDataValue)
	}
	addr := binary.BigEndian.Uint16(pdu[1:3])
	qty := binary.BigEndian.Uint16(pdu[3:5])
	if qty < 1 || qty > 125 {
		return exception(pdu[0], excIllegalDataValue)
	}
	if int(addr)+int(qty) > HoldingMapSize {
		return exception(pdu[0], excIllegalDataAddress)
	}

	// Stale data is as dangerous as no data to a Modbus master polling for
	// live state — fail the read instead of serving frozen values.
	data, age, ok := s.Snapshot.Read()
	if !ok || age > s.staleAfter() {
		return exception(pdu[0], excServerFailure)
	}
	regs := make([]uint16, HoldingMapSize)
	EncodeData(&data, regs)

	out := make([]byte, 2+int(qty)*2)
	out[0] = pdu[0]
	out[1] = byte(qty * 2)
	for i := uint16(0); i < qty; i++ {
		binary.BigEndian.PutUint16(out[2+int(i)*2:], regs[addr+i])
	}
	return out
}

func (s *Server) handleWriteSingle(pdu []byte) []byte {
	if len(pdu) != 5 {
		return exception(pdu[0], excIllegalDataValue)
	}
	addr := binary.BigEndian.Uint16(pdu[1:3])
	val := binary.BigEndian.Uint16(pdu[3:5])
	if err := s.applyWrite(addr, 1, []uint16{val}); err != nil {
		log.Printf("modbus write @%d: %v", addr, err)
		return exception(pdu[0], excIllegalDataAddress)
	}
	out := make([]byte, 5)
	copy(out, pdu[:5]) // echo per spec
	return out
}

func (s *Server) handleWriteMultiple(pdu []byte) []byte {
	if len(pdu) < 6 {
		return exception(pdu[0], excIllegalDataValue)
	}
	addr := binary.BigEndian.Uint16(pdu[1:3])
	qty := binary.BigEndian.Uint16(pdu[3:5])
	bc := pdu[5]
	if int(bc) != int(qty)*2 || len(pdu) != 6+int(bc) {
		return exception(pdu[0], excIllegalDataValue)
	}
	regs := make([]uint16, qty)
	for i := uint16(0); i < qty; i++ {
		regs[i] = binary.BigEndian.Uint16(pdu[6+int(i)*2 : 8+int(i)*2])
	}
	if err := s.applyWrite(addr, qty, regs); err != nil {
		log.Printf("modbus write @%d..%d: %v", addr, addr+qty, err)
		return exception(pdu[0], excIllegalDataAddress)
	}
	out := make([]byte, 5)
	out[0] = pdu[0]
	binary.BigEndian.PutUint16(out[1:3], addr)
	binary.BigEndian.PutUint16(out[3:5], qty)
	return out
}

// applyWrite hands the decode work to CommandSink.Apply, which runs
// the closure under its own mutex. ApplyCommandWrite returns an error
// for partial or unmapped writes; the sink rolls back and we surface
// the error to the Modbus master as exception 02.
func (s *Server) applyWrite(addr, qty uint16, regs []uint16) error {
	return s.Commands.Apply(func(c *shm.PlcCommand) error {
		return ApplyCommandWrite(c, addr, qty, regs)
	})
}

func exception(fn, code byte) []byte {
	return []byte{fn | 0x80, code}
}
