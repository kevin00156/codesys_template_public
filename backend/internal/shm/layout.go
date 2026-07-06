// Package shm: byte-for-byte mirror of the IEC DUT definitions in
// codesys_export/Device/Application/DUT/ShmBridge/.
//
// Every layout change MUST bump the matching Version constant on both sides.
// The reader refuses to mount a segment with an unrecognised version.
//
// pack_mode 8 in IEC means natural 8-byte alignment — same as Go on amd64.
// Sizes are asserted at build time in vet.go.
package shm

import "unsafe"

const (
	PlcDataMagic    uint32 = 0x504C4344 // 'PLCD'
	PlcCommandMagic uint32 = 0x504C4343 // 'PLCC'

	PlcDataVersion    uint16 = 4
	PlcCommandVersion uint16 = 3

	NamePlcData    = "plc_data"
	NamePlcCommand = "plc_cmd"
)

// Header is the first 24 bytes of every segment.
type Header struct {
	Magic   uint32
	Version uint16
	Flags   uint16
	Seq     uint32
	_       uint32
	Cycle   uint64
}

// ─── System ──────────────────────────────────────────────────────────────────

// SystemState is published by the PLC each cycle (PlcData.System).
// 16 bytes.
type SystemState struct {
	Temperature float64 // system temperature °C
	StatusFlags uint32  // bitmask — project-defined
	AlarmFlags  uint32  // bitmask — project-defined
}

// ─── Machine ─────────────────────────────────────────────────────────────────

// AxisState mirrors the key fields from structMC_BasicControl_VisuStatus.
// 48 bytes per axis.
type AxisState struct {
	ActPos  float64 // actual position  (mm / user unit)
	ActVel  float64 // actual velocity  (mm/s)
	SetPos  float64 // command position (mm / user unit)
	SetVel  float64 // command velocity (mm/s)
	Step    int32   // enumAxisControl_Step
	Flags   uint32  // bit0=Enabled bit1=Busy bit2=Error bit3=StandStill bit4=PosLimit bit5=NegLimit
	ErrorID int32   // SMC_ERROR
	_       int32   // reserved
}

// MachineState holds the status of all axes plus machine-level flags.
// 200 bytes.
type MachineState struct {
	Axes     [4]AxisState
	RunState uint32 // machine run state — project-defined enum
	Alarms   uint32 // machine alarm bitmask
}

// AxisCmd carries HMI commands for one axis (PlcCommand.Machine.Axes[i]).
// 32 bytes.
type AxisCmd struct {
	ControlFlags uint32  // bit0=Enable bit1=Home bit2=Reset bit3=Stop bit4=JogPos bit5=JogNeg bit6=MoveAbs
	_            uint32  // reserved
	JogVel       float64 // jog velocity
	MoveAbsPos   float64 // MoveAbsolute target position
	MoveAbsVel   float64 // MoveAbsolute velocity
}

// AxisCmd.ControlFlags bits. The command word is level-held — the PLC acts on
// whatever is set each cycle — so a jog bit left behind by a vanished client
// keeps the axis moving. The jog watchdog in internal/cmdsink clears these two
// when they stop being refreshed.
const (
	AxisCtrlJogPos uint32 = 1 << 4
	AxisCtrlJogNeg uint32 = 1 << 5
)

// MachineCmd carries HMI commands for the whole machine.
// 136 bytes.
type MachineCmd struct {
	Axes         [4]AxisCmd
	ControlFlags uint32 // bit0=Reset bit1=EMS bit2=SystemRun — project-defined
	_            uint32 // reserved
}

// ─── Production ──────────────────────────────────────────────────────────────

// ProductionState is bidirectional: PLC publishes it, HMI can request changes.
// 8 bytes.
type ProductionState struct {
	NProductionState int32
	_                int32 // reserved
}

// ─── Segment structs ─────────────────────────────────────────────────────────

// PlcData is written by the PLC and read by Go.  248 bytes.
type PlcData struct {
	Header     Header          // offset 0
	System     SystemState     // offset 24
	Machine    MachineState    // offset 40
	Production ProductionState // offset 240
}

// PlcCommand is written by Go and read by the PLC.  168 bytes.
type PlcCommand struct {
	Header     Header          // offset 0
	Machine    MachineCmd      // offset 24
	Production ProductionState // offset 160
}

const (
	SizePlcData    = int(unsafe.Sizeof(PlcData{}))
	SizePlcCommand = int(unsafe.Sizeof(PlcCommand{}))
)
