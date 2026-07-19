// Port of backend/internal/shm/golden_test.go.
//
// The golden fixtures pin the byte layout across languages: the Go test
// proves the Go structs encode to testdata/*.bin, rust/shm-bridge's
// go_parity.rs proves the Rust replica matches, and this test proves the C#
// structs produce the identical bytes — from the very same fixture files.
// Regenerate only on a deliberate layout change (Go side: `go test -update`),
// together with a Version bump on every side.

using System.Runtime.InteropServices;
using PlcBridge.Shm;
using Xunit;

namespace PlcBridge.Tests;

public class GoldenTests
{
    // Fills every field with a distinct value; the Go and Rust parity tests
    // build the identical struct from the same formulas.
    private static PlcData GoldenData()
    {
        var d = new PlcData();
        d.Header.Magic = Layout.PlcDataMagic;
        d.Header.Version = Layout.PlcDataVersion;
        d.Header.Flags = 0x5A5A;
        d.Header.Seq = 6;
        d.Header.Cycle = 0x1122334455667788;
        d.System.Temperature = 36.75;
        d.System.StatusFlags = 0xC0FFEE01;
        d.System.AlarmFlags = 0x0BADF00D;
        for (int i = 0; i < 4; i++)
        {
            d.Machine.Axes[i] = new AxisState
            {
                ActPos = 1.5 + 100 * i,
                ActVel = -2.25 + 100 * i,
                SetPos = 3.125 + 100 * i,
                SetVel = -4.0625 + 100 * i,
                Step = 10 + i,
                Flags = (uint)(0x21 + i),
                ErrorID = -(100 + i),
            };
        }
        d.Machine.RunState = 0x00C0FFEE;
        d.Machine.Alarms = 0x0FACE0FF;
        d.Production.NProductionState = -7;
        return d;
    }

    private static PlcCommand GoldenCmd()
    {
        var c = new PlcCommand();
        c.Header.Magic = Layout.PlcCommandMagic;
        c.Header.Version = Layout.PlcCommandVersion;
        c.Header.Flags = 0xA5A5;
        c.Header.Seq = 8;
        c.Header.Cycle = 0x8877665544332211;
        for (int i = 0; i < 4; i++)
        {
            c.Machine.Axes[i] = new AxisCmd
            {
                ControlFlags = (uint)(0x41 + i),
                JogVel = 5.5 + 10 * i,
                MoveAbsPos = -6.25 + 10 * i,
                MoveAbsVel = 7.75 + 10 * i,
            };
        }
        c.Machine.ControlFlags = 5;
        c.Production.NProductionState = 42;
        return c;
    }

    private static byte[] Fixture(string name) =>
        File.ReadAllBytes(Path.Combine(AppContext.BaseDirectory, "testdata", name));

    [Fact]
    public void GoldenPlcData()
    {
        PlcData d = GoldenData();
        byte[] got = MemoryMarshal.AsBytes(MemoryMarshal.CreateSpan(ref d, 1)).ToArray();
        Assert.Equal(Layout.SizePlcData, got.Length);
        Assert.Equal(Fixture("plc_data_v4.bin"), got);
    }

    [Fact]
    public void GoldenPlcCommand()
    {
        PlcCommand c = GoldenCmd();
        byte[] got = MemoryMarshal.AsBytes(MemoryMarshal.CreateSpan(ref c, 1)).ToArray();
        Assert.Equal(Layout.SizePlcCommand, got.Length);
        Assert.Equal(Fixture("plc_cmd_v3.bin"), got);
    }

    [Fact]
    public void LayoutSizesVerify() => Layout.VerifySizes();
}
