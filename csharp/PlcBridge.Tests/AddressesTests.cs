// Port of backend/internal/modbus/addresses_test.go.

using PlcBridge.Modbus;
using PlcBridge.Shm;
using Xunit;

namespace PlcBridge.Tests;

public class AddressesTests
{
    [Fact]
    public void ApplyCommandWrite_SingleField()
    {
        var cmd = new PlcCommand();
        string? err = Addresses.ApplyCommandWrite(ref cmd, Addresses.AddrCmdMachineCtrl, 2, [0x0001, 0x0002]);
        Assert.Null(err);
        Assert.Equal(0x00010002u, cmd.Machine.ControlFlags);
    }

    [Fact]
    public void ApplyCommandWrite_ContiguousMultiFieldSpan()
    {
        var cmd = new PlcCommand();
        // 40..46 covers machine ctrl + axis0 flags + production state.
        string? err = Addresses.ApplyCommandWrite(ref cmd, Addresses.AddrCmdMachineCtrl, 6, [0, 1, 0, 2, 0, 3]);
        Assert.Null(err);
        Assert.Equal(1u, cmd.Machine.ControlFlags);
        Assert.Equal(2u, cmd.Machine.Axes[0].ControlFlags);
        Assert.Equal(3, cmd.Production.NProductionState);
    }

    [Fact]
    public void ApplyCommandWrite_PartialFieldWriteRejected()
    {
        var cmd = new PlcCommand();
        // One register of the two-register machine-ctrl field.
        string? err = Addresses.ApplyCommandWrite(ref cmd, Addresses.AddrCmdMachineCtrl, 1, [7]);
        Assert.NotNull(err);
        Assert.Equal(0u, cmd.Machine.ControlFlags);
    }

    [Fact]
    public void ApplyCommandWrite_UnmappedRegistersRejected()
    {
        var cmd = new PlcCommand();
        // 46.. is past the write map.
        Assert.NotNull(Addresses.ApplyCommandWrite(ref cmd, Addresses.AddrCmdProductionSt + 2, 2, [1, 2]));
        // Read-map addresses are not writable either.
        Assert.NotNull(Addresses.ApplyCommandWrite(ref cmd, Addresses.AddrSysTemperature, 4, new ushort[4]));
    }

    [Fact]
    public void ApplyCommandWrite_SpanLeakingPastMappedFieldsRejected()
    {
        var cmd = new PlcCommand();
        // 40..48: covers all three fields plus two unmapped registers.
        Assert.NotNull(Addresses.ApplyCommandWrite(ref cmd, Addresses.AddrCmdMachineCtrl, 8, new ushort[8]));
    }

    [Fact]
    public void ApplyCommandWrite_QtyAndRegsLengthMustAgree()
    {
        var cmd = new PlcCommand();
        Assert.NotNull(Addresses.ApplyCommandWrite(ref cmd, Addresses.AddrCmdMachineCtrl, 2, [1]));
    }

    [Fact]
    public void EncodeData_RoundTrip()
    {
        var d = new PlcData();
        d.Header.Magic = Layout.PlcDataMagic;
        d.Header.Version = Layout.PlcDataVersion;
        d.Header.Cycle = 0x1122334455667788;
        d.System.Temperature = 36.5;
        d.System.StatusFlags = 0xA0B0C0D0;
        d.Machine.RunState = 3;
        d.Machine.Axes[0].ActPos = -123.456;
        d.Machine.Axes[0].Step = -2;
        d.Production.NProductionState = -7;

        Span<ushort> regs = stackalloc ushort[Addresses.HoldingMapSize];
        Addresses.EncodeData(in d, regs);

        Assert.Equal(0x1122334455667788ul, Addresses.ReadU64(regs, Addresses.AddrCycle));
        Assert.Equal(36.5, Addresses.ReadF64(regs, Addresses.AddrSysTemperature));
        Assert.Equal(0xA0B0C0D0u, Addresses.ReadU32(regs, Addresses.AddrSysStatusFlags));
        Assert.Equal(-123.456, Addresses.ReadF64(regs, Addresses.AddrAxis0ActPos));
        Assert.Equal(-2, (int)Addresses.ReadU32(regs, Addresses.AddrAxis0Step));
        Assert.Equal(-7, (int)Addresses.ReadU32(regs, Addresses.AddrProductionState));
    }
}
