using System.Diagnostics;
using PlcBridge.Shm;

namespace PlcBridge.State;

/// <summary>
/// Holds the most recent successful read of the PLC's data segment, behind a
/// lock so the Modbus and WebSocket servers can read concurrently without
/// coordinating with the shm-poll loop.
/// </summary>
public sealed class Snapshot
{
    private readonly object _mu = new();
    private PlcData _data;
    private long _updatedAt; // Stopwatch timestamp (monotonic)
    private bool _valid;

    public void Update(in PlcData d)
    {
        lock (_mu)
        {
            // Age tracks PLC liveness, not shm-read success: the segment stays
            // perfectly readable after the PLC dies, so a poll loop calling
            // Update with the same frozen data must not look like fresh
            // publishes. Refresh updatedAt only when the producer's cycle
            // counter has advanced.
            if (!_valid || d.Header.Cycle != _data.Header.Cycle)
                _updatedAt = Stopwatch.GetTimestamp();
            _data = d;
            _valid = true;
        }
    }

    /// <summary>
    /// Returns a copy of the latest data, the elapsed time since the PLC last
    /// published a new cycle, and whether any data has been received yet.
    /// </summary>
    public bool Read(out PlcData data, out TimeSpan age)
    {
        lock (_mu)
        {
            data = _data;
            age = Stopwatch.GetElapsedTime(_updatedAt);
            return _valid;
        }
    }
}
