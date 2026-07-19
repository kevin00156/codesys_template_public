// Byte-for-byte mirror of the IEC DUT definitions in
// codesys_export/Device/Application/DUT/ShmBridge/ — the C# counterpart of
// backend/internal/shm/layout.go. One contract, multiple implementations.
//
// Every layout change MUST bump the matching Version constant on every side
// (IEC / Go / C#). The reader refuses to mount a segment with an unrecognised
// version.
//
// pack_mode 8 in IEC means natural 8-byte alignment — the same as
// LayoutKind.Sequential with default packing on x86-64. Sizes are asserted by
// Layout.VerifySizes (the vet.go counterpart), called at startup and from the
// golden-fixture tests.
#pragma warning disable CS0169, CS0649, IDE0051 // reserved padding fields are intentionally unused

using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace PlcBridge.Shm;

/// <summary>Header is the first 24 bytes of every segment.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct Header
{
    public uint Magic;
    public ushort Version;
    public ushort Flags;
    public uint Seq;
    private uint _reserved;
    public ulong Cycle;
}

// ─── System ──────────────────────────────────────────────────────────────────

/// <summary>SystemState is published by the PLC each cycle (PlcData.System). 16 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct SystemState
{
    public double Temperature; // system temperature °C
    public uint StatusFlags;   // bitmask — project-defined
    public uint AlarmFlags;    // bitmask — project-defined
}

// ─── Machine ─────────────────────────────────────────────────────────────────

/// <summary>
/// AxisState mirrors the key fields from structMC_BasicControl_VisuStatus.
/// 48 bytes per axis.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public struct AxisState
{
    public double ActPos; // actual position  (mm / user unit)
    public double ActVel; // actual velocity  (mm/s)
    public double SetPos; // command position (mm / user unit)
    public double SetVel; // command velocity (mm/s)
    public int Step;      // enumAxisControl_Step
    public uint Flags;    // bit0=Enabled bit1=Busy bit2=Error bit3=StandStill bit4=PosLimit bit5=NegLimit
    public int ErrorID;   // SMC_ERROR
    private int _reserved;
}

[InlineArray(4)]
public struct AxisStates
{
    private AxisState _element0;
}

/// <summary>MachineState holds the status of all axes plus machine-level flags. 200 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct MachineState
{
    public AxisStates Axes;
    public uint RunState; // machine run state — project-defined enum
    public uint Alarms;   // machine alarm bitmask
}

/// <summary>AxisCmd carries HMI commands for one axis (PlcCommand.Machine.Axes[i]). 32 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct AxisCmd
{
    public uint ControlFlags; // bit0=Enable bit1=Home bit2=Reset bit3=Stop bit4=JogPos bit5=JogNeg bit6=MoveAbs
    private uint _reserved;
    public double JogVel;     // jog velocity
    public double MoveAbsPos; // MoveAbsolute target position
    public double MoveAbsVel; // MoveAbsolute velocity
}

[InlineArray(4)]
public struct AxisCmds
{
    private AxisCmd _element0;
}

/// <summary>MachineCmd carries HMI commands for the whole machine. 136 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct MachineCmd
{
    public AxisCmds Axes;
    public uint ControlFlags; // bit0=Reset bit1=EMS bit2=SystemRun — project-defined
    private uint _reserved;
}

// ─── Production ──────────────────────────────────────────────────────────────

/// <summary>ProductionState is bidirectional: PLC publishes it, HMI can request changes. 8 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct ProductionState
{
    public int NProductionState;
    private int _reserved;
}

// ─── Segment structs ─────────────────────────────────────────────────────────

/// <summary>PlcData is written by the PLC and read by the bridge. 248 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct PlcData
{
    public Header Header;         // offset 0
    public SystemState System;    // offset 24
    public MachineState Machine;  // offset 40
    public ProductionState Production; // offset 240
}

/// <summary>PlcCommand is written by the bridge and read by the PLC. 168 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct PlcCommand
{
    public Header Header;         // offset 0
    public MachineCmd Machine;    // offset 24
    public ProductionState Production; // offset 160
}

public static class Layout
{
    public const uint PlcDataMagic = 0x504C4344;    // 'PLCD'
    public const uint PlcCommandMagic = 0x504C4343; // 'PLCC'

    public const ushort PlcDataVersion = 4;
    public const ushort PlcCommandVersion = 3;

    public const string NamePlcData = "plc_data";
    public const string NamePlcCommand = "plc_cmd";

    // AxisCmd.ControlFlags bits. The command word is level-held — the PLC acts
    // on whatever is set each cycle — so a jog bit left behind by a vanished
    // client keeps the axis moving. The jog watchdog in CmdSink clears these
    // two when they stop being refreshed.
    public const uint AxisCtrlJogPos = 1u << 4;
    public const uint AxisCtrlJogNeg = 1u << 5;

    public static readonly int SizePlcData = Unsafe.SizeOf<PlcData>();
    public static readonly int SizePlcCommand = Unsafe.SizeOf<PlcCommand>();

    /// <summary>
    /// The vet.go counterpart: C# has no compile-time size assertions, so the
    /// layout contract is verified once at startup (and from unit tests). A
    /// mismatch means the CLR laid the structs out differently from IEC
    /// pack_mode 8 — refuse to run rather than exchange garbage with the PLC.
    /// </summary>
    public static void VerifySizes()
    {
        Check<Header>(24);
        Check<SystemState>(16);
        Check<AxisState>(48);
        Check<AxisCmd>(32);
        Check<MachineState>(200);
        Check<MachineCmd>(136);
        Check<ProductionState>(8);
        Check<PlcData>(248);
        Check<PlcCommand>(168);

        // The seqlock loads Header.Seq through this offset.
        if (SeqOffset() != 8)
            throw new InvalidOperationException($"Header.Seq at offset {SeqOffset()}, expected 8");
    }

    private static void Check<T>(int want) where T : unmanaged
    {
        int got = Unsafe.SizeOf<T>();
        if (got != want)
            throw new InvalidOperationException($"{typeof(T).Name}: size {got} B, expected {want} B — layout drifted from the IEC contract");
    }

    private static nint SeqOffset()
    {
        Header h = default;
        return Unsafe.ByteOffset(ref Unsafe.As<Header, byte>(ref h), ref Unsafe.As<uint, byte>(ref h.Seq));
    }
}
