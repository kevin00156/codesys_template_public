package shm

import (
	"bytes"
	"flag"
	"os"
	"path/filepath"
	"testing"
	"unsafe"
)

// The golden fixtures pin the byte layout across languages: this test proves
// the Go structs encode to testdata/*.bin, and rust/shm-bridge's
// tests/go_parity.rs proves the Rust replica decodes/encodes the same bytes.
// Regenerate (only after a deliberate layout change, together with a Version
// bump on every side):
//
//	go test ./backend/internal/shm -run Golden -update
var update = flag.Bool("update", false, "rewrite golden fixtures")

// goldenData fills every field with a distinct value; the Rust parity test
// builds the identical struct from the same formulas.
func goldenData() PlcData {
	var d PlcData
	d.Header = Header{
		Magic:   PlcDataMagic,
		Version: PlcDataVersion,
		Flags:   0x5A5A,
		Seq:     6,
		Cycle:   0x1122334455667788,
	}
	d.System = SystemState{
		Temperature: 36.75,
		StatusFlags: 0xC0FFEE01,
		AlarmFlags:  0x0BADF00D,
	}
	for i := range d.Machine.Axes {
		d.Machine.Axes[i] = AxisState{
			ActPos:  1.5 + 100*float64(i),
			ActVel:  -2.25 + 100*float64(i),
			SetPos:  3.125 + 100*float64(i),
			SetVel:  -4.0625 + 100*float64(i),
			Step:    int32(10 + i),
			Flags:   uint32(0x21 + i),
			ErrorID: int32(-(100 + i)),
		}
	}
	d.Machine.RunState = 0x00C0FFEE
	d.Machine.Alarms = 0x0FACE0FF
	d.Production.NProductionState = -7
	return d
}

func goldenCmd() PlcCommand {
	var c PlcCommand
	c.Header = Header{
		Magic:   PlcCommandMagic,
		Version: PlcCommandVersion,
		Flags:   0xA5A5,
		Seq:     8,
		Cycle:   0x8877665544332211,
	}
	for i := range c.Machine.Axes {
		c.Machine.Axes[i] = AxisCmd{
			ControlFlags: uint32(0x41 + i),
			JogVel:       5.5 + 10*float64(i),
			MoveAbsPos:   -6.25 + 10*float64(i),
			MoveAbsVel:   7.75 + 10*float64(i),
		}
	}
	c.Machine.ControlFlags = 5
	c.Production.NProductionState = 42
	return c
}

func checkGolden(t *testing.T, name string, got []byte) {
	t.Helper()
	path := filepath.Join("testdata", name)
	if *update {
		if err := os.MkdirAll("testdata", 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, got, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read fixture: %v (regenerate with -update after a deliberate layout change)", err)
	}
	if !bytes.Equal(got, want) {
		t.Errorf("%s: encoding drifted from fixture — layout change without a Version bump?", name)
	}
}

func TestGoldenPlcData(t *testing.T) {
	d := goldenData()
	b := unsafe.Slice((*byte)(unsafe.Pointer(&d)), SizePlcData)
	checkGolden(t, "plc_data_v4.bin", b)
}

func TestGoldenPlcCommand(t *testing.T) {
	c := goldenCmd()
	b := unsafe.Slice((*byte)(unsafe.Pointer(&c)), SizePlcCommand)
	checkGolden(t, "plc_cmd_v3.bin", b)
}
