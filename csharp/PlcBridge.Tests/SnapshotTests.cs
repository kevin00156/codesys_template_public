// Port of backend/internal/state/snapshot_test.go.

using PlcBridge.Shm;
using PlcBridge.State;
using Xunit;

namespace PlcBridge.Tests;

public class SnapshotTests
{
    [Fact]
    public void Snapshot_AgeTracksProducerCycle()
    {
        var s = new Snapshot();
        var d = new PlcData();

        Assert.False(s.Read(out _, out _), "Read ok before any Update");

        d.Header.Cycle = 1;
        s.Update(in d);

        // Re-polling the same frozen segment must not reset the age — the PLC
        // is dead even though the shm read keeps succeeding.
        Thread.Sleep(30);
        s.Update(in d);
        Assert.True(s.Read(out _, out TimeSpan age));
        Assert.True(age >= TimeSpan.FromMilliseconds(25), $"age reset by same-cycle Update: age={age}");

        // A new producer cycle is a fresh publish.
        d.Header.Cycle = 2;
        s.Update(in d);
        s.Read(out _, out age);
        Assert.True(age <= TimeSpan.FromMilliseconds(20), $"age not refreshed by new cycle: {age}");
    }
}
