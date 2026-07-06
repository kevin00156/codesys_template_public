package wsserver

import (
	"time"

	"codesys_dev/backend/internal/shm"
)

// DataMsg is pushed server → client every push interval.
type DataMsg struct {
	Type       string         `json:"type"`  // "data"
	TS         int64          `json:"ts"`    // unix ms
	Stale      bool           `json:"stale"` // snapshot older than the stale threshold — PLC stopped publishing
	AgeMs      int64          `json:"ageMs"` // snapshot age in ms
	System     systemJSON     `json:"system"`
	Machine    machineJSON    `json:"machine"`
	Production productionJSON `json:"production"`
}

type systemJSON struct {
	Temperature float64 `json:"temperature"`
	StatusFlags uint32  `json:"statusFlags"`
	AlarmFlags  uint32  `json:"alarmFlags"`
}

type axisStateJSON struct {
	ActPos  float64 `json:"actPos"`
	ActVel  float64 `json:"actVel"`
	SetPos  float64 `json:"setPos"`
	SetVel  float64 `json:"setVel"`
	Step    int32   `json:"step"`
	Flags   uint32  `json:"flags"`
	ErrorID int32   `json:"errorId"`
}

type machineJSON struct {
	Axes     [4]axisStateJSON `json:"axes"`
	RunState uint32           `json:"runState"`
	Alarms   uint32           `json:"alarms"`
}

type productionJSON struct {
	NProductionState int32 `json:"nProductionState"`
}

// AckMsg is sent back after a command is processed.
type AckMsg struct {
	Type  string `json:"type"` // "ack"
	OK    bool   `json:"ok"`
	Error string `json:"error,omitempty"`
}

// CmdMsg is sent from client → server.
// Type selects the target; unused fields are zero-valued.
type CmdMsg struct {
	Type string `json:"type"` // "machine" | "axis" | "production"

	// "machine": machine-level control
	ControlFlags uint32 `json:"controlFlags"`

	// "axis": per-axis command
	AxisIndex    int     `json:"axisIndex"`
	AxisFlags    uint32  `json:"axisFlags"`
	JogVel       float64 `json:"jogVel"`
	MoveAbsPos   float64 `json:"moveAbsPos"`
	MoveAbsVel   float64 `json:"moveAbsVel"`

	// "production"
	NProductionState int32 `json:"nProductionState"`
}

func dataFromPlc(d *shm.PlcData, age, staleAfter time.Duration) DataMsg {
	var axes [4]axisStateJSON
	for i := range axes {
		a := &d.Machine.Axes[i]
		axes[i] = axisStateJSON{
			ActPos:  a.ActPos,
			ActVel:  a.ActVel,
			SetPos:  a.SetPos,
			SetVel:  a.SetVel,
			Step:    a.Step,
			Flags:   a.Flags,
			ErrorID: a.ErrorID,
		}
	}
	return DataMsg{
		Type:  "data",
		TS:    timeNowMS(),
		Stale: age > staleAfter,
		AgeMs: age.Milliseconds(),
		System: systemJSON{
			Temperature: d.System.Temperature,
			StatusFlags: d.System.StatusFlags,
			AlarmFlags:  d.System.AlarmFlags,
		},
		Machine: machineJSON{
			Axes:     axes,
			RunState: d.Machine.RunState,
			Alarms:   d.Machine.Alarms,
		},
		Production: productionJSON{
			NProductionState: d.Production.NProductionState,
		},
	}
}
