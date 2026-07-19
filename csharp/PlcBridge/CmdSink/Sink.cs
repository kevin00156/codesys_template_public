// Serialises command writes from every source (WebSocket, Modbus) and
// publishes the latest full command struct to /dev/shm/plc_cmd.
//
// It also owns the jog dead-man: the command word is level-held (the PLC acts
// on whatever is set each cycle), so a jog bit whose writer vanished — browser
// crash, WebSocket drop, Modbus master gone — would keep the axis moving
// forever. Clients must re-send the jog command periodically; the watchdog
// clears any axis's jog bits that stop being refreshed.

using System.Diagnostics;
using PlcBridge.Shm;
using PlcBridge.Util;

namespace PlcBridge.CmdSink;

/// <summary>
/// Mutates the pending command. Return null to accept, or an error message to
/// reject — the sink then rolls back every mutation the call made, so a
/// rejected partial write never reaches the PLC.
/// </summary>
public delegate string? CommandMutator(ref PlcCommand cmd);

/// <summary>The same interface is used by the Modbus and WebSocket servers.</summary>
public interface ICommandSink
{
    string? Apply(CommandMutator fn);
}

public sealed class Sink : ICommandSink
{
    /// <summary>Receives each published command (production: Seqlock.WritePlcCommand).</summary>
    public delegate void Publisher(in PlcCommand cmd);

    private readonly object _mu = new();
    private readonly Publisher _publish;
    private PlcCommand _pending;

    // _jogSeen[i] is the last time Apply left axis i with a jog bit set —
    // i.e. the jog was commanded or refreshed. The watchdog clears jog bits
    // older than its timeout. Stopwatch timestamps (monotonic).
    private readonly long[] _jogSeen = new long[4];

    public Sink(Publisher publish)
    {
        _publish = publish;
        _pending.Header.Magic = Layout.PlcCommandMagic;
        _pending.Header.Version = Layout.PlcCommandVersion;
    }

    /// <summary>
    /// Runs fn against the pending command under the sink's lock and publishes
    /// the result. A non-null error from fn rolls everything back.
    /// </summary>
    public string? Apply(CommandMutator fn)
    {
        lock (_mu)
        {
            PlcCommand saved = _pending;
            string? err = fn(ref _pending);
            if (err != null)
            {
                _pending = saved;
                return err;
            }
            long now = Stopwatch.GetTimestamp();
            for (int i = 0; i < 4; i++)
            {
                if ((_pending.Machine.Axes[i].ControlFlags & (Layout.AxisCtrlJogPos | Layout.AxisCtrlJogNeg)) != 0)
                    _jogSeen[i] = now;
            }
            PublishLocked();
            return null;
        }
    }

    private void PublishLocked()
    {
        _pending.Header.Cycle++;
        _publish(in _pending);
    }

    /// <summary>
    /// Clears any axis's jog bits that have not been refreshed (via Apply)
    /// within timeout, and republishes. Runs until ct is cancelled.
    /// </summary>
    public void StartJogWatchdog(CancellationToken ct, TimeSpan timeout)
    {
        _ = Task.Run(async () =>
        {
            try
            {
                using var tick = new PeriodicTimer(timeout / 4);
                while (await tick.WaitForNextTickAsync(ct))
                    ExpireJogs(Stopwatch.GetTimestamp(), timeout);
            }
            catch (OperationCanceledException)
            {
            }
            catch (Exception e)
            {
                // The watchdog is a safety mechanism: if it dies (bad timeout,
                // I/O failure in the log path, anything), jog bits would never
                // expire and an axis could keep moving after its client
                // vanished. Fail the whole process loudly — Go's equivalent
                // (a ticker panic) does the same — and let systemd restart it.
                BridgeLog.Print($"cmdsink: jog watchdog died: {e.Message}");
                Environment.Exit(1);
            }
        }, CancellationToken.None);
    }

    internal void ExpireJogs(long now, TimeSpan timeout)
    {
        long timeoutTicks = (long)(timeout.TotalSeconds * Stopwatch.Frequency);
        lock (_mu)
        {
            bool expired = false;
            for (int i = 0; i < 4; i++)
            {
                ref AxisCmd a = ref _pending.Machine.Axes[i];
                if ((a.ControlFlags & (Layout.AxisCtrlJogPos | Layout.AxisCtrlJogNeg)) == 0)
                    continue;
                if (now - _jogSeen[i] < timeoutTicks)
                    continue;
                a.ControlFlags &= ~(Layout.AxisCtrlJogPos | Layout.AxisCtrlJogNeg);
                expired = true;
                BridgeLog.Print($"cmdsink: jog watchdog cleared axis {i} (no refresh for {GoDuration.Format(timeout)})");
            }
            if (expired)
                PublishLocked();
        }
    }
}
