package modbus

import (
	"testing"

	"codesys_dev/backend/internal/shm"
)

func TestApplyCommandWrite(t *testing.T) {
	t.Run("single field", func(t *testing.T) {
		var cmd shm.PlcCommand
		if err := ApplyCommandWrite(&cmd, AddrCmdMachineCtrl, 2, []uint16{0x0001, 0x0002}); err != nil {
			t.Fatalf("ApplyCommandWrite: %v", err)
		}
		if cmd.Machine.ControlFlags != 0x00010002 {
			t.Errorf("ControlFlags: got %#x want 0x10002", cmd.Machine.ControlFlags)
		}
	})

	t.Run("contiguous multi-field span", func(t *testing.T) {
		var cmd shm.PlcCommand
		// 40..46 covers machine ctrl + axis0 flags + production state.
		regs := []uint16{0, 1, 0, 2, 0, 3}
		if err := ApplyCommandWrite(&cmd, AddrCmdMachineCtrl, 6, regs); err != nil {
			t.Fatalf("ApplyCommandWrite: %v", err)
		}
		if cmd.Machine.ControlFlags != 1 || cmd.Machine.Axes[0].ControlFlags != 2 || cmd.Production.NProductionState != 3 {
			t.Errorf("fields: ctrl=%d axis0=%d prod=%d, want 1/2/3",
				cmd.Machine.ControlFlags, cmd.Machine.Axes[0].ControlFlags, cmd.Production.NProductionState)
		}
	})

	t.Run("partial field write rejected", func(t *testing.T) {
		var cmd shm.PlcCommand
		// One register of the two-register machine-ctrl field.
		if err := ApplyCommandWrite(&cmd, AddrCmdMachineCtrl, 1, []uint16{7}); err == nil {
			t.Fatal("want error for partial field write")
		}
		if cmd.Machine.ControlFlags != 0 {
			t.Errorf("rejected write mutated cmd: %#x", cmd.Machine.ControlFlags)
		}
	})

	t.Run("unmapped registers rejected", func(t *testing.T) {
		var cmd shm.PlcCommand
		// 46.. is past the write map.
		if err := ApplyCommandWrite(&cmd, AddrCmdProductionSt+2, 2, []uint16{1, 2}); err == nil {
			t.Fatal("want error for unmapped write")
		}
		// Read-map addresses are not writable either.
		if err := ApplyCommandWrite(&cmd, AddrSysTemperature, 4, make([]uint16, 4)); err == nil {
			t.Fatal("want error for write into read map")
		}
	})

	t.Run("span leaking past mapped fields rejected", func(t *testing.T) {
		var cmd shm.PlcCommand
		// 40..48: covers all three fields plus two unmapped registers.
		if err := ApplyCommandWrite(&cmd, AddrCmdMachineCtrl, 8, make([]uint16, 8)); err == nil {
			t.Fatal("want error for span covering unmapped registers")
		}
	})

	t.Run("qty and regs length must agree", func(t *testing.T) {
		var cmd shm.PlcCommand
		if err := ApplyCommandWrite(&cmd, AddrCmdMachineCtrl, 2, []uint16{1}); err == nil {
			t.Fatal("want error for qty != len(regs)")
		}
	})
}

func TestEncodeData_RoundTrip(t *testing.T) {
	var d shm.PlcData
	d.Header.Magic = shm.PlcDataMagic
	d.Header.Version = shm.PlcDataVersion
	d.Header.Cycle = 0x1122334455667788
	d.System.Temperature = 36.5
	d.System.StatusFlags = 0xA0B0C0D0
	d.Machine.RunState = 3
	d.Machine.Axes[0].ActPos = -123.456
	d.Machine.Axes[0].Step = -2
	d.Production.NProductionState = -7

	regs := make([]uint16, HoldingMapSize)
	EncodeData(&d, regs)

	if got := readU64(regs, AddrCycle); got != d.Header.Cycle {
		t.Errorf("cycle: got %#x", got)
	}
	if got := readF64(regs, AddrSysTemperature); got != 36.5 {
		t.Errorf("temperature: got %v", got)
	}
	if got := readU32(regs, AddrSysStatusFlags); got != 0xA0B0C0D0 {
		t.Errorf("status flags: got %#x", got)
	}
	if got := readF64(regs, AddrAxis0ActPos); got != -123.456 {
		t.Errorf("axis0 pos: got %v", got)
	}
	if got := int32(readU32(regs, AddrAxis0Step)); got != -2 {
		t.Errorf("axis0 step: got %d", got)
	}
	if got := int32(readU32(regs, AddrProductionState)); got != -7 {
		t.Errorf("production state: got %d", got)
	}
}
