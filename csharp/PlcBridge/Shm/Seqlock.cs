using System.Runtime.CompilerServices;

namespace PlcBridge.Shm;

public enum SeqlockResult
{
    Ok,
    Busy,            // seqlock: writer kept us spinning
    MagicMismatch,   // wrong segment or layout
    VersionMismatch, // rebuild PLC and bridge with matching layout
}

/// <summary>
/// The seqlock protocol over a shared-memory segment — the C# counterpart of
/// backend/internal/shm/reader.go and writer.go. The protocol (odd = write in
/// progress, even = stable, retry on seq change) and its documented
/// memory-ordering constraints are identical across the Go / Rust / C#
/// implementations; only the atomic primitives differ.
/// </summary>
public static class Seqlock
{
    internal const int MaxRetries = 100;

    /// <summary>Log-parity error strings (reader.go's sentinel errors).</summary>
    public static string Message(SeqlockResult r) => r switch
    {
        SeqlockResult.Busy => "seqlock: writer kept us spinning",
        SeqlockResult.MagicMismatch => "magic mismatch: wrong segment or layout",
        SeqlockResult.VersionMismatch => "version mismatch: rebuild PLC and bridge with matching layout",
        _ => "ok",
    };

    /// <summary>
    /// Copies the segment into dst under the seqlock protocol. On Ok,
    /// dst.Header.Magic and dst.Header.Version are guaranteed to match this
    /// build's expectations.
    ///
    /// Memory-ordering caveat: only Header.Seq is accessed atomically
    /// (Volatile.Read = acquire per ECMA-335); the payload is a plain bulk
    /// copy. Acquire ordering stops the payload's loads from moving above the
    /// first seq load, but nothing stops the hardware from reordering them
    /// past the second seq load — that requires a read fence this code does
    /// not issue. On x86/amd64 (TSO: loads are not reordered with loads) this
    /// is sound, and amd64 is the only deploy target — same constraint as the
    /// Go implementation (see reader.go). Porting to ARM needs
    /// Interlocked.MemoryBarrier() before the second seq load or per-word
    /// atomic copies. The Seq accesses themselves must stay Volatile: they
    /// also prevent the JIT from reordering across them.
    /// </summary>
    public static unsafe SeqlockResult ReadPlcData(Mapping m, out PlcData dst)
    {
        dst = default;
        var src = (PlcData*)m.Ptr;
        ref uint seq = ref Unsafe.AsRef<uint>(&src->Header.Seq);
        for (int i = 0; i < MaxRetries; i++)
        {
            uint s1 = Volatile.Read(ref seq);
            if ((s1 & 1) != 0)
                continue;
            dst = *src;
            uint s2 = Volatile.Read(ref seq);
            if (s1 != s2)
                continue;
            if (dst.Header.Magic != Layout.PlcDataMagic)
                return SeqlockResult.MagicMismatch;
            if (dst.Header.Version != Layout.PlcDataVersion)
                return SeqlockResult.VersionMismatch;
            return SeqlockResult.Ok;
        }
        return SeqlockResult.Busy;
    }

    /// <summary>
    /// Publishes src to the segment under the seqlock protocol. Caller must
    /// initialise src.Header.Magic/Version before the first call (Sink's
    /// constructor does this). Adding fields to PlcCommand requires no changes
    /// here.
    ///
    /// The writer — not the caller — owns Header.Seq. The bulk copy below
    /// would otherwise overwrite the segment's seq with src.Header.Seq (an
    /// even value), briefly making the segment look "stable" while the payload
    /// is still being written, so a reader could latch a torn snapshot. We
    /// force the copied seq to the in-progress (odd) value to keep the seqlock
    /// invariant: seq stays odd for the entire time the payload is in flux.
    ///
    /// Memory-ordering caveat: the payload copy is plain stores between the
    /// two Volatile.Write (release) seq stores. Release ordering stops the
    /// payload's stores from moving below the final seq store, but not from
    /// moving above the first one. Sound on x86/amd64 (TSO: stores are not
    /// reordered with stores), which is the only deploy target — see the
    /// matching note on ReadPlcData before porting to ARM.
    /// </summary>
    public static unsafe void WritePlcCommand(Mapping m, in PlcCommand src)
    {
        var dst = (PlcCommand*)m.Ptr;
        ref uint seq = ref Unsafe.AsRef<uint>(&dst->Header.Seq);
        uint s = Volatile.Read(ref seq);
        // Crash recovery: if the previous writer process died mid-write, the
        // segment's seq is still odd. A plain s+1 would then be even — the
        // "write in progress" marker would look stable and a reader could
        // latch a torn snapshot. Skip ahead to the next odd value instead.
        uint odd = s + 1 + (s & 1);
        Volatile.Write(ref seq, odd); // odd: write in progress

        PlcCommand tmp = src;
        tmp.Header.Seq = odd; // copy carries the in-progress seq, never an even one
        *dst = tmp;

        Volatile.Write(ref seq, odd + 1); // even: stable
    }
}
