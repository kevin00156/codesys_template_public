// Port of backend/internal/shm/seqlock_concurrent_test.go.
//
// This test deliberately races a writer against a reader on the same segment —
// that is what the seqlock is for. The payload copy is a benign race by
// design.

using PlcBridge.Shm;
using Xunit;

namespace PlcBridge.Tests;

public class SeqlockConcurrentTests
{
    // A contention smoke test, not a proof. The writer keeps two payload
    // fields equal on every write; a reader that ever observes them unequal
    // has latched a torn snapshot. It exercises the full read/write protocol
    // under real parallelism and catches gross breakage (seq never settling
    // even, reader never stabilising, payload skew).
    //
    // It does NOT reliably reproduce the WritePlcCommand seq-clobber
    // regression on its own: the reader's s1!=s2 check masks the transient
    // even seq in all but a vanishingly rare interleaving. The real protection
    // for that case is the writer keeping seq odd throughout the copy — see
    // the comment in Seqlock.WritePlcCommand.
    [Fact]
    public void Seqlock_NoTornReads()
    {
        using var m = Mapping.InMemory(Layout.SizePlcCommand);

        bool stop = false;
        var writer = new Thread(() =>
        {
            var src = new PlcCommand();
            src.Header.Magic = Layout.PlcCommandMagic;
            src.Header.Version = Layout.PlcCommandVersion;
            for (uint i = 1; i <= 2_000_000; i++)
            {
                // Two header fields that must always agree within a single
                // snapshot (Flags is the low 16 bits of Cycle).
                src.Header.Flags = (ushort)i;
                src.Header.Cycle = i;
                Seqlock.WritePlcCommand(m, in src);
            }
            Volatile.Write(ref stop, true);
        });
        writer.Start();

        int torn = 0;
        int reads = 0;
        while (!Volatile.Read(ref stop))
        {
            if (!TestSegments.ReadCmd(m, out PlcCommand got))
                continue;
            reads++;
            if (got.Header.Flags != (ushort)got.Header.Cycle)
                torn++;
        }
        writer.Join();

        Assert.True(torn == 0, $"observed {torn} torn reads out of {reads} — seqlock invariant violated");
    }
}
