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

	_ = uint(64 - unsafe.Sizeof(TraceHeader{}))
	_ = uint(unsafe.Sizeof(TraceHeader{}) - 64)

	_ = uint(56 - unsafe.Sizeof(TraceAxisSample{}))
	_ = uint(unsafe.Sizeof(TraceAxisSample{}) - 56)

	_ = uint(256 - unsafe.Sizeof(TraceSample{}))
	_ = uint(unsafe.Sizeof(TraceSample{}) - 256)

	// The ring reader loads WriteIdx atomically through this offset.
	_ = uint(32 - unsafe.Offsetof(TraceHeader{}.WriteIdx))
	_ = uint(unsafe.Offsetof(TraceHeader{}.WriteIdx) - 32)

	_ = uint(32 - unsafe.Offsetof(TraceSample{}.Axes))
	_ = uint(unsafe.Offsetof(TraceSample{}.Axes) - 32)
)
