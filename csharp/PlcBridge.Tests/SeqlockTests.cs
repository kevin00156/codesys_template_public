// Port of backend/internal/shm/seqlock_test.go.

using PlcBridge.Shm;
using Xunit;

namespace PlcBridge.Tests;

public class SeqlockTests
{
    [Fact]
    public unsafe void ReadPlcData_RoundTrip()
    {
        using var m = Mapping.InMemory(Layout.SizePlcData);
        var seg = (PlcData*)m.Ptr;
        seg->Header.Magic = Layout.PlcDataMagic;
        seg->Header.Version = Layout.PlcDataVersion;
        seg->Header.Seq = 2; // even == stable
        seg->Header.Cycle = 42;

        Assert.Equal(SeqlockResult.Ok, Seqlock.ReadPlcData(m, out PlcData dst));
        Assert.Equal(42ul, dst.Header.Cycle);
    }

    [Fact]
    public unsafe void ReadPlcData_RejectsMagic()
    {
        using var m = Mapping.InMemory(Layout.SizePlcData);
        var seg = (PlcData*)m.Ptr;
        seg->Header.Magic = 0xBADBAD;
        seg->Header.Version = Layout.PlcDataVersion;
        seg->Header.Seq = 2;

        Assert.Equal(SeqlockResult.MagicMismatch, Seqlock.ReadPlcData(m, out _));
    }

    [Fact]
    public unsafe void ReadPlcData_RejectsVersion()
    {
        using var m = Mapping.InMemory(Layout.SizePlcData);
        var seg = (PlcData*)m.Ptr;
        seg->Header.Magic = Layout.PlcDataMagic;
        seg->Header.Version = Layout.PlcDataVersion + 99;
        seg->Header.Seq = 2;

        Assert.Equal(SeqlockResult.VersionMismatch, Seqlock.ReadPlcData(m, out _));
    }

    [Fact]
    public unsafe void ReadPlcData_RejectsBusy()
    {
        using var m = Mapping.InMemory(Layout.SizePlcData);
        var seg = (PlcData*)m.Ptr;
        seg->Header.Magic = Layout.PlcDataMagic;
        seg->Header.Version = Layout.PlcDataVersion;
        seg->Header.Seq = 1; // odd forever -> writer perpetually in progress

        Assert.Equal(SeqlockResult.Busy, Seqlock.ReadPlcData(m, out _));
    }

    [Fact]
    public void WritePlcCommand_RoundTrip()
    {
        using var m = Mapping.InMemory(Layout.SizePlcCommand);

        var src = new PlcCommand();
        src.Header.Magic = Layout.PlcCommandMagic;
        src.Header.Version = Layout.PlcCommandVersion;
        src.Header.Seq = 999; // garbage: the writer owns seq and must ignore this
        src.Header.Cycle = 7;

        Seqlock.WritePlcCommand(m, in src);

        Assert.True(TestSegments.ReadCmd(m, out PlcCommand got), "readCmd: seqlock never stabilised");
        Assert.Equal(7ul, got.Header.Cycle);
        Assert.Equal(Layout.PlcCommandMagic, got.Header.Magic);
        Assert.Equal(Layout.PlcCommandVersion, got.Header.Version);
        // Writer controls seq: first write must land on the first even value,
        // not anything derived from src.Header.Seq (999).
        Assert.Equal(2u, got.Header.Seq);

        // Second write advances seq by exactly 2 and stays even.
        src.Header.Cycle = 8;
        Seqlock.WritePlcCommand(m, in src);
        Assert.True(TestSegments.ReadCmd(m, out PlcCommand got2), "readCmd: seqlock never stabilised on 2nd write");
        Assert.Equal(4u, got2.Header.Seq);
        Assert.Equal(8ul, got2.Header.Cycle);
    }

    // If the previous writer process died mid-write, the segment's seq is left
    // odd. The next write must not flip the "in progress" marker to an even
    // value (a reader could latch a torn snapshot); it must skip ahead to the
    // next odd value and finish even.
    [Fact]
    public unsafe void WritePlcCommand_RecoversFromOddSeq()
    {
        using var m = Mapping.InMemory(Layout.SizePlcCommand);
        var seg = (PlcCommand*)m.Ptr;
        seg->Header.Seq = 7; // crashed mid-write

        var src = new PlcCommand();
        src.Header.Magic = Layout.PlcCommandMagic;
        src.Header.Version = Layout.PlcCommandVersion;
        src.Header.Cycle = 11;

        Seqlock.WritePlcCommand(m, in src);

        Assert.True(TestSegments.ReadCmd(m, out PlcCommand got), "readCmd: seqlock never stabilised after odd-seq recovery");
        Assert.True((got.Header.Seq & 1) == 0, $"seq still odd after write: {got.Header.Seq}");
        Assert.True(got.Header.Seq > 7, $"seq did not advance past the stale value: got {got.Header.Seq}");
        Assert.Equal(11ul, got.Header.Cycle);
    }
}
