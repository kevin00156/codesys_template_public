package shm

import "unsafe"

const (
	_ = uint(24 - unsafe.Sizeof(Header{}))
	_ = uint(unsafe.Sizeof(Header{}) - 24)

	_ = uint(16 - unsafe.Sizeof(SystemState{}))
	_ = uint(unsafe.Sizeof(SystemState{}) - 16)

	_ = uint(48 - unsafe.Sizeof(AxisState{}))
	_ = uint(unsafe.Sizeof(AxisState{}) - 48)

	_ = uint(32 - unsafe.Sizeof(AxisCmd{}))
	_ = uint(unsafe.Sizeof(AxisCmd{}) - 32)

	_ = uint(200 - unsafe.Sizeof(MachineState{}))
	_ = uint(unsafe.Sizeof(MachineState{}) - 200)

	_ = uint(136 - unsafe.Sizeof(MachineCmd{}))
	_ = uint(unsafe.Sizeof(MachineCmd{}) - 136)

	_ = uint(8 - unsafe.Sizeof(ProductionState{}))
	_ = uint(unsafe.Sizeof(ProductionState{}) - 8)

	_ = uint(248 - unsafe.Sizeof(PlcData{}))
	_ = uint(unsafe.Sizeof(PlcData{}) - 248)

	_ = uint(168 - unsafe.Sizeof(PlcCommand{}))
	_ = uint(unsafe.Sizeof(PlcCommand{}) - 168)
)
