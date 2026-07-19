using System.IO.MemoryMappedFiles;
using System.Runtime.InteropServices;

namespace PlcBridge.Shm;

/// <summary>
/// A view over a shared-memory segment.
///
/// In production it is an mmap of /dev/shm/&lt;name&gt; via MemoryMappedFile.
/// Tests construct an in-memory Mapping with <see cref="InMemory"/> (the
/// counterpart of the Go side's mappingFromBytes) — 8-byte aligned native
/// memory, so the seqlock can be exercised without touching /dev/shm.
/// </summary>
public sealed unsafe class Mapping : IDisposable
{
    private readonly MemoryMappedFile? _file;
    private readonly MemoryMappedViewAccessor? _view;
    private readonly void* _native; // in-memory variant; null for real mmaps

    public byte* Ptr { get; }
    public int Size { get; }

    private Mapping(MemoryMappedFile file, MemoryMappedViewAccessor view, byte* ptr, int size)
    {
        _file = file;
        _view = view;
        Ptr = ptr;
        Size = size;
    }

    private Mapping(void* native, int size)
    {
        _native = native;
        Ptr = (byte*)native;
        Size = size;
    }

    /// <summary>
    /// Maps a read-only view of /dev/shm/&lt;name&gt;.
    /// Use for plc_data: the PLC is the sole writer; the bridge only reads.
    /// </summary>
    public static Mapping OpenRead(string name, int size) => OpenShm(name, size, readOnly: true);

    /// <summary>
    /// Maps a read-write view of /dev/shm/&lt;name&gt;.
    /// Use for plc_cmd: the bridge writes commands, the PLC reads them.
    /// </summary>
    public static Mapping Open(string name, int size) => OpenShm(name, size, readOnly: false);

    private static Mapping OpenShm(string name, int size, bool readOnly)
    {
        string path = "/dev/shm/" + name;
        FileStream f;
        try
        {
            f = new FileStream(path, FileMode.Open,
                readOnly ? FileAccess.Read : FileAccess.ReadWrite,
                FileShare.ReadWrite | FileShare.Delete);
        }
        catch (Exception e)
        {
            throw new IOException($"open {path}: {e.Message}");
        }

        long len;
        try
        {
            len = f.Length;
        }
        catch (Exception e)
        {
            f.Dispose();
            throw new IOException($"stat {path}: {e.Message}");
        }
        if (len < size)
        {
            f.Dispose();
            throw new IOException($"{path}: segment {len} B, expected at least {size} B — PLC layout out of date?");
        }

        var access = readOnly ? MemoryMappedFileAccess.Read : MemoryMappedFileAccess.ReadWrite;
        MemoryMappedFile mmf;
        MemoryMappedViewAccessor view;
        try
        {
            mmf = MemoryMappedFile.CreateFromFile(f, null, size, access, HandleInheritability.None, leaveOpen: false);
            view = mmf.CreateViewAccessor(0, size, access);
        }
        catch (Exception e)
        {
            f.Dispose();
            throw new IOException($"mmap {path}: {e.Message}");
        }

        byte* p = null;
        view.SafeMemoryMappedViewHandle.AcquirePointer(ref p);
        return new Mapping(mmf, view, p, size);
    }

    /// <summary>In-memory segment for tests: 8-byte aligned, zeroed.</summary>
    public static Mapping InMemory(int size)
    {
        void* mem = NativeMemory.AlignedAlloc((nuint)size, 8);
        NativeMemory.Clear(mem, (nuint)size);
        return new Mapping(mem, size);
    }

    public void Dispose()
    {
        if (_view != null)
        {
            _view.SafeMemoryMappedViewHandle.ReleasePointer();
            _view.Dispose();
        }
        _file?.Dispose();
        if (_native != null)
            NativeMemory.AlignedFree(_native);
    }
}
