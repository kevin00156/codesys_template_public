// Encodes the PLC snapshot as a Modbus holding-register bank — the C# mirror
// of backend/internal/modbus/addresses.go. This file is the single source of
// truth for the register layout on the C# side; keep it line-for-line in sync
// with the Go side.
//
// Word ordering: big-endian (high word at lower address).
//
// Adding a register:
//  1. pick the next free address (mind field width)
//  2. add an Addr* constant
//  3. add a line to EncodeData (read) or CommandFields (write)

using PlcBridge.Shm;

namespace PlcBridge.Modbus;

public static class Addresses
{
    public const int HoldingMapSize = 64;

    // Read map (PLC → Modbus master).
    public const ushort AddrMagic = 0;            // 1 reg
    public const ushort AddrDataVersion = 1;      // 1 reg
    public const ushort AddrCycle = 2;            // 4 regs (uint64)
    public const ushort AddrSysTemperature = 8;   // 4 regs (float64)
    public const ushort AddrSysStatusFlags = 12;  // 2 regs (uint32)
    public const ushort AddrSysAlarmFlags = 14;   // 2 regs (uint32)
    public const ushort AddrMachineRunState = 16; // 2 regs (uint32)
    public const ushort AddrMachineAlarms = 18;   // 2 regs (uint32)
    public const ushort AddrAxis0ActPos = 20;     // 4 regs (float64)
    public const ushort AddrAxis0ActVel = 24;     // 4 regs (float64)
    public const ushort AddrAxis0Step = 28;       // 2 regs (int32)
    public const ushort AddrAxis0Flags = 30;      // 2 regs (uint32)
    public const ushort AddrProductionState = 32; // 2 regs (int32)

    // Write map (Modbus master → PLC).
    public const ushort AddrCmdMachineCtrl = 40;  // 2 regs (uint32) — MachineCmd.ControlFlags
    public const ushort AddrCmdAxis0Flags = 42;   // 2 regs (uint32) — Axes[0].ControlFlags
    public const ushort AddrCmdProductionSt = 44; // 2 regs (int32)  — ProductionState

    public static void EncodeData(in PlcData d, Span<ushort> regs)
    {
        regs[AddrMagic] = (ushort)(d.Header.Magic & 0xFFFF);
        regs[AddrDataVersion] = d.Header.Version;
        PutU64(regs, AddrCycle, d.Header.Cycle);

        PutF64(regs, AddrSysTemperature, d.System.Temperature);
        PutU32(regs, AddrSysStatusFlags, d.System.StatusFlags);
        PutU32(regs, AddrSysAlarmFlags, d.System.AlarmFlags);

        PutU32(regs, AddrMachineRunState, d.Machine.RunState);
        PutU32(regs, AddrMachineAlarms, d.Machine.Alarms);

        PutF64(regs, AddrAxis0ActPos, d.Machine.Axes[0].ActPos);
        PutF64(regs, AddrAxis0ActVel, d.Machine.Axes[0].ActVel);
        PutU32(regs, AddrAxis0Step, (uint)d.Machine.Axes[0].Step);
        PutU32(regs, AddrAxis0Flags, d.Machine.Axes[0].Flags);

        PutU32(regs, AddrProductionState, (uint)d.Production.NProductionState);
    }

    private delegate void FieldApply(ref PlcCommand cmd, ReadOnlySpan<ushort> words);

    private readonly record struct WritableField(ushort Addr, ushort Width, FieldApply Apply);

    private static readonly WritableField[] CommandFields =
    [
        new(AddrCmdMachineCtrl, 2, static (ref PlcCommand c, ReadOnlySpan<ushort> w) =>
            c.Machine.ControlFlags = ReadU32(w, 0)),
        new(AddrCmdAxis0Flags, 2, static (ref PlcCommand c, ReadOnlySpan<ushort> w) =>
            c.Machine.Axes[0].ControlFlags = ReadU32(w, 0)),
        new(AddrCmdProductionSt, 2, static (ref PlcCommand c, ReadOnlySpan<ushort> w) =>
            c.Production.NProductionState = (int)ReadU32(w, 0)),
    ];

    /// <summary>
    /// Decodes a Modbus write into command-struct mutations. Returns null on
    /// success, or an error message for partial or unmapped writes — the sink
    /// rolls back and the server surfaces exception 02 to the master.
    /// </summary>
    public static string? ApplyCommandWrite(ref PlcCommand cmd, ushort addr, ushort qty, ReadOnlySpan<ushort> regs)
    {
        if (qty != regs.Length)
            return $"regs length {regs.Length} != qty {qty}";
        ushort end = unchecked((ushort)(addr + qty));
        ushort covered = 0;
        foreach (var f in CommandFields)
        {
            ushort fEnd = (ushort)(f.Addr + f.Width);
            if (fEnd <= addr || end <= f.Addr)
                continue;
            if (f.Addr < addr || end < fEnd)
                return $"partial write to field at {f.Addr} (width {f.Width})";
            int offset = f.Addr - addr;
            f.Apply(ref cmd, regs.Slice(offset, f.Width));
            covered += f.Width;
        }
        if (covered != qty)
            return $"write at {addr}..{end} covers unmapped registers";
        return null;
    }

    internal static void PutU32(Span<ushort> regs, ushort addr, uint v)
    {
        regs[addr] = (ushort)(v >> 16);
        regs[addr + 1] = (ushort)v;
    }

    internal static void PutU64(Span<ushort> regs, ushort addr, ulong v)
    {
        regs[addr] = (ushort)(v >> 48);
        regs[addr + 1] = (ushort)(v >> 32);
        regs[addr + 2] = (ushort)(v >> 16);
        regs[addr + 3] = (ushort)v;
    }

    internal static void PutF64(Span<ushort> regs, ushort addr, double v) =>
        PutU64(regs, addr, BitConverter.DoubleToUInt64Bits(v));

    internal static uint ReadU32(ReadOnlySpan<ushort> regs, ushort addr) =>
        ((uint)regs[addr] << 16) | regs[addr + 1];

    internal static ulong ReadU64(ReadOnlySpan<ushort> regs, ushort addr) =>
        ((ulong)regs[addr] << 48) | ((ulong)regs[addr + 1] << 32) |
        ((ulong)regs[addr + 2] << 16) | regs[addr + 3];

    internal static double ReadF64(ReadOnlySpan<ushort> regs, ushort addr) =>
        BitConverter.UInt64BitsToDouble(ReadU64(regs, addr));
}
