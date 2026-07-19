// Serves the WebSocket endpoint that pushes PlcData snapshots to all
// connected clients and accepts command messages — the C# counterpart of
// backend/internal/wsserver/server.go.

using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Threading.Channels;
using PlcBridge.CmdSink;
using PlcBridge.Shm;
using PlcBridge.State;

namespace PlcBridge.Ws;

public sealed class WsServer
{
    public required Snapshot Snapshot { get; init; }
    public ICommandSink? Commands { get; init; }
    public TimeSpan Interval { get; init; }   // push interval; defaults to 100ms
    public TimeSpan StaleAfter { get; init; } // snapshot age past which data is flagged stale; defaults to 500ms

    /// <summary>
    /// If set, gates command (write) messages. It is evaluated once per
    /// connection against the upgrade request — which carries the session
    /// cookie — so an unauthenticated socket can still receive the live data
    /// push but every command it sends is rejected with an "unauthorized" ack.
    /// null => every connection may write (auth-disabled / dev posture).
    /// </summary>
    public Func<HttpContext, bool>? AuthorizeWrite { get; set; }

    /// <summary>
    /// Process-shutdown token. Kestrel's graceful shutdown waits for in-flight
    /// requests — including upgraded WebSockets — for the full
    /// HostOptions.ShutdownTimeout (30s default) before aborting them, so a
    /// dashboard that stays connected would stall every restart by 30s.
    /// Linking each connection to this token tears it down immediately on
    /// SIGTERM, matching the Go bridge's sub-second exit.
    /// </summary>
    public CancellationToken Shutdown { get; init; }

    private static readonly TimeSpan WriteWait = TimeSpan.FromSeconds(5);      // per-write deadline
    private static readonly TimeSpan KeepAliveInterval = TimeSpan.FromSeconds(30);

    internal static readonly JsonSerializerOptions JsonOpts = new()
    {
        DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull,
        PropertyNameCaseInsensitive = true, // Go's encoding/json matches case-insensitively
    };

    private TimeSpan EffectiveInterval =>
        Interval > TimeSpan.Zero ? Interval : TimeSpan.FromMilliseconds(100);

    private TimeSpan EffectiveStaleAfter =>
        StaleAfter > TimeSpan.Zero ? StaleAfter : TimeSpan.FromMilliseconds(500);

    public async Task Handle(HttpContext ctx)
    {
        if (!ctx.WebSockets.IsWebSocketRequest)
        {
            ctx.Response.StatusCode = StatusCodes.Status400BadRequest;
            return;
        }

        // Same-origin wall (gorilla/websocket's CheckOrigin default, which
        // ASP.NET Core does not provide): the Origin host must match the
        // request Host; non-browser clients without an Origin header pass.
        // This blocks cross-site WebSocket hijacking — a page on another site
        // can't open our telemetry/command socket with the operator's ambient
        // session.
        string? origin = ctx.Request.Headers.Origin;
        if (!string.IsNullOrEmpty(origin))
        {
            // Compare the RAW authority (host[:port] exactly as sent), not
            // Uri.Authority — the latter strips default ports, which would
            // accept "http://evil-form.example:80" against Host "…" in ways
            // gorilla's raw u.Host comparison does not.
            if (!Uri.TryCreate(origin, UriKind.Absolute, out _) ||
                !string.Equals(RawAuthority(origin), ctx.Request.Host.Value, StringComparison.OrdinalIgnoreCase))
            {
                ctx.Response.StatusCode = StatusCodes.Status403Forbidden;
                return;
            }
        }

        // Decide write permission from the session before the upgrade hijacks
        // the request. Reads (the periodic data push) are never gated.
        bool canWrite = AuthorizeWrite == null || AuthorizeWrite(ctx);

        // .NET 8's server WebSocket has no ping-with-deadline API (that
        // arrived in .NET 9); KeepAliveInterval sends unsolicited pong
        // heartbeats so middleboxes keep the connection alive. Stalled-client
        // protection comes from the per-write deadline below: a full TCP
        // buffer times the write out and tears the connection down.
        using WebSocket ws = await ctx.WebSockets.AcceptWebSocketAsync(
            new WebSocketAcceptContext { KeepAliveInterval = KeepAliveInterval });

        using var cts = CancellationTokenSource.CreateLinkedTokenSource(ctx.RequestAborted, Shutdown);

        // The WebSocket forbids concurrent writers. All writes — periodic data
        // pushes and command acks — are serialised through this gate.
        var sendMu = new SemaphoreSlim(1, 1);

        async Task<bool> SendJson(object msg)
        {
            // Serialization stays INSIDE the try: a payload that cannot be
            // serialized (the PLC can publish NaN/Inf doubles, which JSON
            // rejects) must tear the connection down like any other write
            // failure — Go's WriteJSON error path — not fault the push task
            // and leave a zombie connection that acks but never pushes.
            try
            {
                byte[] payload = JsonSerializer.SerializeToUtf8Bytes(msg, msg.GetType(), JsonOpts);
                using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cts.Token);
                deadline.CancelAfter(WriteWait);
                await sendMu.WaitAsync(deadline.Token);
                try
                {
                    await ws.SendAsync(payload, WebSocketMessageType.Text, true, deadline.Token);
                }
                finally
                {
                    sendMu.Release();
                }
                return true;
            }
            catch
            {
                cts.Cancel(); // unblock the reader so it tears down
                return false;
            }
        }

        // Non-blocking ack queue: DropWrite mirrors Go's `select { default: }` —
        // if the writer is gone or backed up we drop the ack rather than
        // deadlock; the next receive sees the closed conn and breaks.
        var acks = Channel.CreateBounded<AckMsg>(new BoundedChannelOptions(4)
        {
            FullMode = BoundedChannelFullMode.DropWrite,
        });

        var pushTask = Task.Run(async () =>
        {
            try
            {
                using var tick = new PeriodicTimer(EffectiveInterval);
                while (await tick.WaitForNextTickAsync(cts.Token))
                {
                    if (!Snapshot.Read(out var d, out var age))
                        continue;
                    // Keep pushing stale data (the dashboard shows the last
                    // known values) but flag it, so a dead PLC doesn't
                    // masquerade as live.
                    if (!await SendJson(DataMsg.FromPlc(in d, age, EffectiveStaleAfter)))
                        return;
                }
            }
            catch (OperationCanceledException)
            {
            }
        });

        var ackTask = Task.Run(async () =>
        {
            try
            {
                await foreach (AckMsg ack in acks.Reader.ReadAllAsync(cts.Token))
                {
                    if (!await SendJson(ack))
                        return;
                }
            }
            catch (OperationCanceledException)
            {
            }
        });

        try
        {
            var buf = new byte[4096];
            while (true)
            {
                string? raw = await ReadTextMessage(ws, buf, cts.Token);
                if (raw == null)
                    break;
                var ack = new AckMsg { Ok = true };
                if (!canWrite)
                {
                    ack.Ok = false;
                    ack.Error = "unauthorized";
                }
                else if (!TryParseCmd(raw, out CmdMsg cmd))
                {
                    ack.Ok = false;
                    ack.Error = "invalid json";
                }
                else
                {
                    string? err = ApplyCmd(cmd);
                    if (err != null)
                    {
                        ack.Ok = false;
                        ack.Error = err;
                    }
                }
                acks.Writer.TryWrite(ack);
            }
        }
        catch
        {
        }
        cts.Cancel();
        await Task.WhenAll(pushTask, ackTask);
    }

    /// <summary>Assembles one text message; null on close/error.</summary>
    private static async Task<string?> ReadTextMessage(WebSocket ws, byte[] buf, CancellationToken ct)
    {
        var sb = new MemoryStream();
        while (true)
        {
            WebSocketReceiveResult r;
            try
            {
                r = await ws.ReceiveAsync(buf, ct);
            }
            catch
            {
                return null;
            }
            if (r.MessageType == WebSocketMessageType.Close)
                return null;
            sb.Write(buf, 0, r.Count);
            if (r.EndOfMessage)
                return Encoding.UTF8.GetString(sb.GetBuffer(), 0, (int)sb.Length);
        }
    }

    /// <summary>host[:port] exactly as written in an absolute URL, no default-port stripping.</summary>
    private static string RawAuthority(string url)
    {
        int i = url.IndexOf("://", StringComparison.Ordinal);
        string rest = i >= 0 ? url[(i + 3)..] : url;
        int slash = rest.IndexOfAny(['/', '?', '#']);
        return slash >= 0 ? rest[..slash] : rest;
    }

    private static bool TryParseCmd(string raw, out CmdMsg cmd)
    {
        try
        {
            // The JSON literal "null" decodes to the zero command (Go's
            // Unmarshal no-op), which then acks `unknown command type ""` —
            // only malformed JSON is an "invalid json" ack.
            cmd = JsonSerializer.Deserialize<CmdMsg>(raw, JsonOpts) ?? new CmdMsg();
            return true;
        }
        catch (JsonException)
        {
            cmd = new CmdMsg();
            return false;
        }
    }

    internal string? ApplyCmd(CmdMsg cmd)
    {
        if (Commands == null)
            return "command sink unavailable (PLC shm not mounted)";
        return Commands.Apply((ref PlcCommand c) =>
        {
            switch (cmd.Type)
            {
                case "machine":
                    c.Machine.ControlFlags = cmd.ControlFlags;
                    break;
                case "axis":
                    if (cmd.AxisIndex >= 0 && cmd.AxisIndex < 4)
                    {
                        ref AxisCmd a = ref c.Machine.Axes[cmd.AxisIndex];
                        a.ControlFlags = cmd.AxisFlags;
                        a.JogVel = cmd.JogVel;
                        a.MoveAbsPos = cmd.MoveAbsPos;
                        a.MoveAbsVel = cmd.MoveAbsVel;
                    }
                    break;
                case "production":
                    c.Production.NProductionState = cmd.NProductionState;
                    break;
                default:
                    return $"unknown command type \"{cmd.Type}\"";
            }
            return null;
        });
    }
}
