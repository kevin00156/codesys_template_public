// Port of backend/internal/cmdsink/cmdsink_test.go.

using System.Diagnostics;
using PlcBridge.CmdSink;
using PlcBridge.Shm;
using Xunit;

namespace PlcBridge.Tests;

public class CmdSinkTests
{
    // Records every published command.
    private sealed class Capture
    {
        public PlcCommand Last;
        public int Count;

        public void Publish(in PlcCommand cmd)
        {
            Last = cmd;
            Count++;
        }
    }

    private static long Ticks(TimeSpan t) => (long)(t.TotalSeconds * Stopwatch.Frequency);

    [Fact]
    public void Apply_PublishesAndCounts()
    {
        var pub = new Capture();
        var s = new Sink(pub.Publish);

        string? err = s.Apply((ref PlcCommand c) =>
        {
            c.Machine.ControlFlags = 4;
            return null;
        });
        Assert.Null(err);
        Assert.Equal(1, pub.Count);
        Assert.Equal(4u, pub.Last.Machine.ControlFlags);
        Assert.Equal(Layout.PlcCommandMagic, pub.Last.Header.Magic);
        Assert.Equal(Layout.PlcCommandVersion, pub.Last.Header.Version);
        Assert.Equal(1ul, pub.Last.Header.Cycle);
    }

    [Fact]
    public void Apply_RollsBackOnError()
    {
        var pub = new Capture();
        var s = new Sink(pub.Publish);

        string? err = s.Apply((ref PlcCommand c) =>
        {
            c.Machine.ControlFlags = 99; // partial mutation that must not survive
            return "rejected";
        });
        Assert.NotNull(err);
        Assert.Equal(0, pub.Count);

        // The pending state must be clean for the next writer.
        Assert.Null(s.Apply((ref PlcCommand c) => null));
        Assert.Equal(0u, pub.Last.Machine.ControlFlags);
    }

    [Fact]
    public void JogWatchdog_ClearsUnrefreshedJog()
    {
        var pub = new Capture();
        var s = new Sink(pub.Publish);

        Assert.Null(s.Apply((ref PlcCommand c) =>
        {
            c.Machine.Axes[1].ControlFlags = Layout.AxisCtrlJogPos;
            c.Machine.Axes[1].JogVel = 5;
            return null;
        }));

        var timeout = TimeSpan.FromMilliseconds(20);
        // Not yet expired: nothing happens.
        s.ExpireJogs(Stopwatch.GetTimestamp(), timeout);
        Assert.Equal(1, pub.Count);

        // Past the timeout: jog bits cleared and republished.
        s.ExpireJogs(Stopwatch.GetTimestamp() + Ticks(timeout) * 2, timeout);
        Assert.Equal(2, pub.Count);
        Assert.Equal(0u, pub.Last.Machine.Axes[1].ControlFlags & (Layout.AxisCtrlJogPos | Layout.AxisCtrlJogNeg));
    }

    [Fact]
    public void JogWatchdog_RefreshKeepsJogAlive()
    {
        var pub = new Capture();
        var s = new Sink(pub.Publish);

        void Jog() => Assert.Null(s.Apply((ref PlcCommand c) =>
        {
            c.Machine.Axes[0].ControlFlags = Layout.AxisCtrlJogNeg;
            return null;
        }));

        var timeout = TimeSpan.FromMilliseconds(50);
        Jog();
        Thread.Sleep(timeout / 2);
        Jog(); // client keepalive re-send
        s.ExpireJogs(Stopwatch.GetTimestamp(), timeout);
        Assert.NotEqual(0u, pub.Last.Machine.Axes[0].ControlFlags & Layout.AxisCtrlJogNeg);

        // Non-jog commands on another axis must not refresh axis 0's jog.
        Thread.Sleep(timeout + timeout / 2);
        s.ExpireJogs(Stopwatch.GetTimestamp(), timeout);
        Assert.Equal(0u, pub.Last.Machine.Axes[0].ControlFlags & Layout.AxisCtrlJogNeg);
    }
}
