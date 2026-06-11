// Package modbus encodes the PLC snapshot as a Modbus holding-register
// bank. This file is the single source of truth for the register layout.
//
// Word ordering: big-endian (high word at lower address).
//
// Adding a register:
//   1. pick the next free address (mind field width)
//   2. add an Addr* constant
//   3. add a line to EncodeData (read) or commandFields (write)
package modbus

import (
	"fmt"
	"math"

	"codesys_dev/backend/internal/shm"
)

const HoldingMapSize = 64

// Read map (PLC → Modbus master).
const (
	AddrMagic           uint16 = 0  // 1 reg
	AddrDataVersion     uint16 = 1  // 1 reg
	AddrCycle           uint16 = 2  // 4 regs (uint64)
	AddrSysTemperature  uint16 = 8  // 4 regs (float64)
	AddrSysStatusFlags  uint16 = 12 // 2 regs (uint32)
	AddrSysAlarmFlags   uint16 = 14 // 2 regs (uint32)
	AddrMachineRunState uint16 = 16 // 2 regs (uint32)
	AddrMachineAlarms   uint16 = 18 // 2 regs (uint32)
	AddrAxis0ActPos     uint16 = 20 // 4 regs (float64)
	AddrAxis0ActVel     uint16 = 24 // 4 regs (float64)
	AddrAxis0Step       uint16 = 28 // 2 regs (int32)
	AddrAxis0Flags      uint16 = 30 // 2 regs (uint32)
	AddrProductionState uint16 = 32 // 2 regs (int32)
)

// Write map (Modbus master → PLC).
const (
	AddrCmdMachineCtrl  uint16 = 40 // 2 regs (uint32) — MachineCmd.ControlFlags
	AddrCmdAxis0Flags   uint16 = 42 // 2 regs (uint32) — Axes[0].ControlFlags
	AddrCmdProductionSt uint16 = 44 // 2 regs (int32)  — ProductionState
)

func EncodeData(d *shm.PlcData, regs []uint16) {
	regs[AddrMagic] = uint16(d.Header.Magic & 0xFFFF)
	regs[AddrDataVersion] = d.Header.Version
	putU64(regs, AddrCycle, d.Header.Cycle)

	putF64(regs, AddrSysTemperature, d.System.Temperature)
	putU32(regs, AddrSysStatusFlags, d.System.StatusFlags)
	putU32(regs, AddrSysAlarmFlags, d.System.AlarmFlags)

	putU32(regs, AddrMachineRunState, d.Machine.RunState)
	putU32(regs, AddrMachineAlarms, d.Machine.Alarms)

	putF64(regs, AddrAxis0ActPos, d.Machine.Axes[0].ActPos)
	putF64(regs, AddrAxis0ActVel, d.Machine.Axes[0].ActVel)
	putU32(regs, AddrAxis0Step, uint32(d.Machine.Axes[0].Step))
	putU32(regs, AddrAxis0Flags, d.Machine.Axes[0].Flags)

	putU32(regs, AddrProductionState, uint32(d.Production.NProductionState))
}

var commandFields = []writableField{
	{AddrCmdMachineCtrl, 2, func(c *shm.PlcCommand, w []uint16) {
		c.Machine.ControlFlags = readU32(w, 0)
	}},
	{AddrCmdAxis0Flags, 2, func(c *shm.PlcCommand, w []uint16) {
		c.Machine.Axes[0].ControlFlags = readU32(w, 0)
	}},
	{AddrCmdProductionSt, 2, func(c *shm.PlcCommand, w []uint16) {
		c.Production.NProductionState = int32(readU32(w, 0))
	}},
}

type writableField struct {
	addr  uint16
	width uint16
	apply func(cmd *shm.PlcCommand, words []uint16)
}

func ApplyCommandWrite(cmd *shm.PlcCommand, addr, qty uint16, regs []uint16) error {
	if int(qty) != len(regs) {
		return fmt.Errorf("regs length %d != qty %d", len(regs), qty)
	}
	end := addr + qty
	covered := uint16(0)
	for _, f := range commandFields {
		fEnd := f.addr + f.width
		if fEnd <= addr || end <= f.addr {
			continue
		}
		if f.addr < addr || end < fEnd {
			return fmt.Errorf("partial write to field at %d (width %d)", f.addr, f.width)
		}
		offset := f.addr - addr
		f.apply(cmd, regs[offset:offset+f.width])
		covered += f.width
	}
	if covered != qty {
		return fmt.Errorf("write at %d..%d covers unmapped registers", addr, end)
	}
	return nil
}

func putU32(regs []uint16, addr uint16, v uint32) {
	regs[addr] = uint16(v >> 16)
	regs[addr+1] = uint16(v)
}
func putU64(regs []uint16, addr uint16, v uint64) {
	regs[addr] = uint16(v >> 48)
	regs[addr+1] = uint16(v >> 32)
	regs[addr+2] = uint16(v >> 16)
	regs[addr+3] = uint16(v)
}
func putF64(regs []uint16, addr uint16, v float64) {
	putU64(regs, addr, math.Float64bits(v))
}
func readU32(regs []uint16, addr uint16) uint32 {
	return uint32(regs[addr])<<16 | uint32(regs[addr+1])
}
func readU64(regs []uint16, addr uint16) uint64 {
	return uint64(regs[addr])<<48 | uint64(regs[addr+1])<<32 |
		uint64(regs[addr+2])<<16 | uint64(regs[addr+3])
}
func readF64(regs []uint16, addr uint16) float64 {
	return math.Float64frombits(readU64(regs, addr))
}
