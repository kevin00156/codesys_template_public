// WebSocket message DTOs — the JSON wire format is field-for-field identical
// to backend/internal/wsserver/message.go; the Svelte frontend must not be
// able to tell which bridge implementation it is talking to.

using System.Text.Json.Serialization;
using PlcBridge.Shm;

namespace PlcBridge.Ws;

/// <summary>DataMsg is pushed server → client every push interval.</summary>
public sealed class DataMsg
{
    [JsonPropertyName("type")] public string Type { get; set; } = "data";
    [JsonPropertyName("ts")] public long Ts { get; set; }        // unix ms
    [JsonPropertyName("stale")] public bool Stale { get; set; }  // snapshot older than the stale threshold — PLC stopped publishing
    [JsonPropertyName("ageMs")] public long AgeMs { get; set; }  // snapshot age in ms
    [JsonPropertyName("system")] public SystemJson System { get; set; } = new();
    [JsonPropertyName("machine")] public MachineJson Machine { get; set; } = new();
    [JsonPropertyName("production")] public ProductionJson Production { get; set; } = new();

    public static DataMsg FromPlc(in PlcData d, TimeSpan age, TimeSpan staleAfter)
    {
        var axes = new AxisStateJson[4];
        for (int i = 0; i < 4; i++)
        {
            AxisState a = d.Machine.Axes[i];
            axes[i] = new AxisStateJson
            {
                ActPos = a.ActPos,
                ActVel = a.ActVel,
                SetPos = a.SetPos,
                SetVel = a.SetVel,
                Step = a.Step,
                Flags = a.Flags,
                ErrorId = a.ErrorID,
            };
        }
        return new DataMsg
        {
            Ts = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(),
            Stale = age > staleAfter,
            AgeMs = (long)age.TotalMilliseconds,
            System = new SystemJson
            {
                Temperature = d.System.Temperature,
                StatusFlags = d.System.StatusFlags,
                AlarmFlags = d.System.AlarmFlags,
            },
            Machine = new MachineJson
            {
                Axes = axes,
                RunState = d.Machine.RunState,
                Alarms = d.Machine.Alarms,
            },
            Production = new ProductionJson
            {
                NProductionState = d.Production.NProductionState,
            },
        };
    }
}

public sealed class SystemJson
{
    [JsonPropertyName("temperature")] public double Temperature { get; set; }
    [JsonPropertyName("statusFlags")] public uint StatusFlags { get; set; }
    [JsonPropertyName("alarmFlags")] public uint AlarmFlags { get; set; }
}

public sealed class AxisStateJson
{
    [JsonPropertyName("actPos")] public double ActPos { get; set; }
    [JsonPropertyName("actVel")] public double ActVel { get; set; }
    [JsonPropertyName("setPos")] public double SetPos { get; set; }
    [JsonPropertyName("setVel")] public double SetVel { get; set; }
    [JsonPropertyName("step")] public int Step { get; set; }
    [JsonPropertyName("flags")] public uint Flags { get; set; }
    [JsonPropertyName("errorId")] public int ErrorId { get; set; }
}

public sealed class MachineJson
{
    [JsonPropertyName("axes")] public AxisStateJson[] Axes { get; set; } = [];
    [JsonPropertyName("runState")] public uint RunState { get; set; }
    [JsonPropertyName("alarms")] public uint Alarms { get; set; }
}

public sealed class ProductionJson
{
    [JsonPropertyName("nProductionState")] public int NProductionState { get; set; }
}

/// <summary>AckMsg is sent back after a command is processed.</summary>
public sealed class AckMsg
{
    [JsonPropertyName("type")] public string Type { get; set; } = "ack";
    [JsonPropertyName("ok")] public bool Ok { get; set; }
    [JsonPropertyName("error")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Error { get; set; }
}

/// <summary>
/// CmdMsg is sent from client → server.
/// Type selects the target; unused fields are zero-valued.
/// </summary>
public sealed class CmdMsg
{
    [JsonPropertyName("type")] public string Type { get; set; } = "";

    // "machine": machine-level control
    [JsonPropertyName("controlFlags")] public uint ControlFlags { get; set; }

    // "axis": per-axis command
    [JsonPropertyName("axisIndex")] public int AxisIndex { get; set; }
    [JsonPropertyName("axisFlags")] public uint AxisFlags { get; set; }
    [JsonPropertyName("jogVel")] public double JogVel { get; set; }
    [JsonPropertyName("moveAbsPos")] public double MoveAbsPos { get; set; }
    [JsonPropertyName("moveAbsVel")] public double MoveAbsVel { get; set; }

    // "production"
    [JsonPropertyName("nProductionState")] public int NProductionState { get; set; }
}
