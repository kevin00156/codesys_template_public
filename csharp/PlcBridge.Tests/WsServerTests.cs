// Port of backend/internal/wsserver/server_test.go.

using PlcBridge.CmdSink;
using PlcBridge.Shm;
using PlcBridge.State;
using PlcBridge.Ws;
using Xunit;

namespace PlcBridge.Tests;

public class WsServerTests
{
    // Applies the mutator against an in-memory command struct so ApplyCmd can
    // be exercised without a real shm mapping.
    private sealed class FakeSink : ICommandSink
    {
        public PlcCommand Cmd;

        public string? Apply(CommandMutator fn) => fn(ref Cmd);
    }

    [Fact]
    public void ApplyCmd()
    {
        var s = new WsServer { Snapshot = new Snapshot(), Commands = new FakeSink() };

        Assert.Null(s.ApplyCmd(new CmdMsg { Type = "machine" }));
        Assert.Null(s.ApplyCmd(new CmdMsg { Type = "axis", AxisIndex = 0 }));
        Assert.Null(s.ApplyCmd(new CmdMsg { Type = "production" }));
        Assert.NotNull(s.ApplyCmd(new CmdMsg { Type = "bogus" }));
    }

    [Fact]
    public void ApplyCmd_NilSink()
    {
        // A bridge running without shm mounted has a null command sink;
        // commands must error rather than crash.
        var s = new WsServer { Snapshot = new Snapshot() };
        Assert.NotNull(s.ApplyCmd(new CmdMsg { Type = "machine" }));
    }
}
