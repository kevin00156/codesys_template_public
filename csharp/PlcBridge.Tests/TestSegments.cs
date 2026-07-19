using System.Runtime.CompilerServices;
using PlcBridge.Shm;

namespace PlcBridge.Tests;

/// <summary>
/// In-memory segment helpers — the counterpart of the Go tests'
/// dataMapping/cmdMapping/readCmd (seqlock_test.go).
/// </summary>
internal static class TestSegments
{
    /// <summary>
    /// A test-local seqlock reader for the command segment (the bridge never
    /// reads plc_cmd in production — the PLC does — so there is no
    /// ReadPlcCommand).
    /// </summary>
    public static unsafe bool ReadCmd(Mapping m, out PlcCommand dst)
    {
        dst = default;
        var src = (PlcCommand*)m.Ptr;
        ref uint seq = ref Unsafe.AsRef<uint>(&src->Header.Seq);
        for (int i = 0; i < Seqlock.MaxRetries; i++)
        {
            uint s1 = Volatile.Read(ref seq);
            if ((s1 & 1) != 0)
                continue;
            dst = *src;
            uint s2 = Volatile.Read(ref seq);
            if (s1 == s2)
                return true;
        }
        dst = default;
        return false;
    }
}
