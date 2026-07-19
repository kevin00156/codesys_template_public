// plc_bridge (C# implementation): read PLC data from /dev/shm, hold a
// snapshot, serve it over Modbus TCP and a WebSocket/HTTP API.
//
// Same contract as the Go implementation in backend/cmd/plc_bridge — same shm
// layout, same wire formats, same flags — so the e2e smoke harness and the
// Svelte frontend work unchanged against either bridge.
//
// Pin to a non-isolated CPU when running on the CODESYS Edge box:
//
//	taskset -c 0 dotnet PlcBridge.dll
//
// CPU 2-3 are reserved for the PLC runtime by the kernel cmdline
// (isolcpus=2-3 nohz_full=2-3).

using System.Runtime.InteropServices;
using System.Security.Cryptography.X509Certificates;
using Microsoft.Extensions.FileProviders;
using PlcBridge.Auth;
using PlcBridge.CmdSink;
using PlcBridge.Modbus;
using PlcBridge.Shm;
using PlcBridge.State;
using PlcBridge.Util;
using PlcBridge.Ws;

// The template's built-in fallback login, applied only when no
// PLC_BRIDGE_*_HASH is set in the environment. It exists so a fresh clone
// boots with a known password (the read-only dashboard stays open; only
// machine control needs login) and the UI can nag you to change it.
// CHANGE IT for any real deployment: mint a hash with `plc_bridge -gen-hash`
// and set PLC_BRIDGE_PASSWORD_HASH (see README / docs/DEVELOPMENT.md §4).
const string DefaultVendorPassword = "111111";

var flags = new Flags("plc_bridge");
var modbusAddr = flags.String("modbus", "127.0.0.1:5020", "Modbus TCP listen address; the Modbus write map has no authentication, so bind a non-loopback address (e.g. :5020) only on a firewalled/dedicated machine network");
var httpAddr = flags.String("http", ":8443", "HTTP/WebSocket listen address");
var tlsCert = flags.String("tls-cert", "", "TLS certificate file (enables HTTPS; auto-detected from ./cert.pem if empty)");
var tlsKey = flags.String("tls-key", "", "TLS private key file (auto-detected from ./key.pem if empty)");
var pollInterval = flags.Duration("poll", TimeSpan.FromMilliseconds(10), "shm poll interval");
var pushInterval = flags.Duration("push", TimeSpan.FromMilliseconds(100), "WebSocket push interval");
var jogTimeout = flags.Duration("jog-timeout", TimeSpan.FromMilliseconds(500), "dead-man timeout: jog bits not refreshed within this window are cleared");
var staleAfter = flags.Duration("stale-after", TimeSpan.FromMilliseconds(500), "snapshot age past which data is flagged stale (WS) / reads fail (Modbus)");
var genHash = flags.Bool("gen-hash", false, "read a password from stdin, print its bcrypt hash for the PLC_BRIDGE_*_HASH env vars, then exit");
var webroot = flags.String("webroot", "", "directory with the built frontend (auto-detected from ./frontend/dist if empty; the Go bridge embeds it at compile time instead)");
flags.Parse(args);

// Password-hash helper: `plc_bridge -gen-hash` reads one line from stdin and
// prints a bcrypt hash. Mint each role password this way and store the hash
// in the service env file — the plaintext never touches args, shell history,
// or git.
if (genHash.Value)
{
    string pw = (Console.In.ReadToEnd()).TrimEnd('\r', '\n');
    try
    {
        Console.WriteLine(Authenticator.HashPassword(pw));
    }
    catch (ArgumentException e)
    {
        BridgeLog.Fatal($"gen-hash: {e.Message}"); // e.g. password over bcrypt's 72-byte limit
    }
    return 0;
}

// Refuse to run if the CLR laid the shm structs out differently from the IEC
// contract (the vet.go counterpart — C# has no compile-time size asserts).
Layout.VerifySizes();

// TLS auto-enables when cert.pem/key.pem sit in the working directory, so a
// deploy that drops certs in needs no flag change (mirrors the systemd unit).
string certFile = tlsCert.Value, keyFile = tlsKey.Value;
// Path.Exists (not File.Exists) matches Go's os.Stat: a directory named
// cert.pem still triggers the auto-detect and then fails loudly at cert load,
// instead of silently starting a plain-HTTP bridge.
if (certFile == "" && keyFile == "" && Path.Exists("cert.pem") && Path.Exists("key.pem"))
    (certFile, keyFile) = ("cert.pem", "key.pem");
// Half a TLS config is a misconfiguration, not a fallback: silently serving
// plain HTTP would send passwords in clear while logging "https" and marking
// the session cookie Secure (which the browser then never returns).
if ((certFile == "") != (keyFile == ""))
    BridgeLog.Fatal($"TLS misconfigured: got cert=\"{certFile}\" key=\"{keyFile}\" — provide both -tls-cert and -tls-key (or neither)");

Mapping dataMap;
try
{
    dataMap = Mapping.OpenRead(Layout.NamePlcData, Layout.SizePlcData);
}
catch (IOException e)
{
    BridgeLog.Fatal($"open {Layout.NamePlcData}: {e.Message}");
    return 1;
}

Mapping cmdMap;
try
{
    cmdMap = Mapping.Open(Layout.NamePlcCommand, Layout.SizePlcCommand);
}
catch (IOException e)
{
    BridgeLog.Fatal($"open {Layout.NamePlcCommand}: {e.Message}");
    return 1;
}

var snap = new Snapshot();
var sink = new Sink((in PlcCommand c) => Seqlock.WritePlcCommand(cmdMap, in c));

using var cts = new CancellationTokenSource();

// SIGINT/SIGTERM → graceful shutdown, mirroring the Go bridge's signal.Notify.
void OnSignal(PosixSignalContext sctx)
{
    sctx.Cancel = true;
    BridgeLog.Print("signal received, shutting down");
    cts.Cancel();
}
using var sigint = PosixSignalRegistration.Create(PosixSignal.SIGINT, OnSignal);
using var sigterm = PosixSignalRegistration.Create(PosixSignal.SIGTERM, OnSignal);

// Dead-man for the level-held jog bits: clients (HMI, Modbus master) must
// re-send jog commands periodically; bits that stop being refreshed are
// cleared so a vanished client can't leave an axis moving.
sink.StartJogWatchdog(cts.Token, jogTimeout.Value);

// shm poll loop.
var pollTask = Task.Run(async () =>
{
    using var tick = new PeriodicTimer(pollInterval.Value);
    string? lastErr = null;
    try
    {
        while (await tick.WaitForNextTickAsync(cts.Token))
        {
            SeqlockResult res = Seqlock.ReadPlcData(dataMap, out PlcData d);
            if (res != SeqlockResult.Ok)
            {
                string msg = Seqlock.Message(res);
                if (msg != lastErr)
                    BridgeLog.Print($"read plc_data: {msg}");
                lastErr = msg;
                continue;
            }
            if (lastErr != null)
            {
                BridgeLog.Print("read plc_data: recovered");
                lastErr = null;
            }
            snap.Update(in d);
        }
    }
    catch (OperationCanceledException)
    {
    }
    catch (Exception e)
    {
        // A dead poll loop must not leave a healthy-looking bridge serving
        // stale nothing — fail loudly and take the process down (Go's ticker
        // panic equivalent).
        BridgeLog.Print($"poll: {e.Message}");
        cts.Cancel();
    }
});

// Modbus TCP server.
var modbusSrv = new ModbusServer { Snapshot = snap, Commands = sink, StaleAfter = staleAfter.Value };
_ = Task.Run(async () =>
{
    try
    {
        await modbusSrv.ListenAndServe(modbusAddr.Value, cts.Token);
    }
    catch (OperationCanceledException)
    {
    }
    catch (Exception e)
    {
        BridgeLog.Print($"modbus: {e.Message}");
        cts.Cancel();
    }
});

// HTTP/HTTPS server: WebSocket API + frontend static files.
var wsSrv = new WsServer
{
    Snapshot = snap,
    Commands = sink,
    Interval = pushInterval.Value,
    StaleAfter = staleAfter.Value,
    Shutdown = cts.Token, // SIGTERM tears WS connections down immediately (Go parity)
};

var builder = WebApplication.CreateBuilder(new WebApplicationOptions { Args = [] });
builder.Logging.ClearProviders(); // one log stream, Go-style, via BridgeLog
System.Net.IPAddress? httpHost;
int httpPort;
try
{
    (httpHost, httpPort) = NetAddr.Parse(httpAddr.Value);
}
catch (FormatException e)
{
    BridgeLog.Fatal($"http: {e.Message}");
    return 1;
}
builder.WebHost.ConfigureKestrel(k =>
{
    void Configure(Microsoft.AspNetCore.Server.Kestrel.Core.ListenOptions lo)
    {
        if (certFile != "")
        {
            // Serve the FULL chain from cert.pem. Go's tls.LoadX509KeyPair
            // loads every CERTIFICATE block; CreateFromPemFile alone takes
            // only the first, which would leave strict clients unable to
            // verify a standard fullchain (leaf+intermediate) deployment.
            var chain = new X509Certificate2Collection();
            chain.ImportFromPemFile(certFile);
            lo.UseHttps(o =>
            {
                o.ServerCertificate = X509Certificate2.CreateFromPemFile(certFile, keyFile);
                o.ServerCertificateChain = chain;
            });
        }
    }
    if (httpHost == null)
        k.ListenAnyIP(httpPort, Configure);
    else
        k.Listen(httpHost, httpPort, Configure);
});
var app = builder.Build();

// Role-password auth. Hashes come from the environment; if none is set the
// template falls back to a built-in DEFAULT vendor password so a fresh clone
// runs with a known login and an on-screen "change me" nag. The session
// cookie is marked Secure only when we serve TLS.
string vendorHash = Environment.GetEnvironmentVariable("PLC_BRIDGE_PASSWORD_HASH") ?? "";   // vendor (highest tier)
string tunerHash = Environment.GetEnvironmentVariable("PLC_BRIDGE_TUNER_HASH") ?? "";       // tuner
string operatorHash = Environment.GetEnvironmentVariable("PLC_BRIDGE_OPERATOR_HASH") ?? ""; // operator
bool usingDefault = false;
if (vendorHash == "" && tunerHash == "" && operatorHash == "")
{
    vendorHash = Authenticator.HashPassword(DefaultVendorPassword);
    usingDefault = true;
}
var authn = new Authenticator(vendorHash, tunerHash, operatorHash, certFile != "");
if (usingDefault)
    authn.UseDefaultPassword(DefaultVendorPassword); // surfaced via /api/auth/status

app.UseWebSockets();

// Go's ServeMux patterns are exact: /api/login/ or /ws/ never match a route
// and fall through to the file server's 404. ASP.NET treats the trailing
// slash as optional, so short-circuit here to keep the bridges
// indistinguishable on the wire.
app.Use(async (ctx, next) =>
{
    string p = ctx.Request.Path.Value ?? "";
    if (p.Length > 1 && p.EndsWith('/') && (p.StartsWith("/api/") || p == "/ws/"))
    {
        ctx.Response.StatusCode = StatusCodes.Status404NotFound;
        ctx.Response.ContentType = "text/plain; charset=utf-8";
        ctx.Response.Headers.XContentTypeOptions = "nosniff";
        await ctx.Response.WriteAsync("404 page not found\n");
        return;
    }
    await next();
});

// Control commands flow over the WebSocket, not HTTP, so writes are gated
// there: AuthorizeWrite requires a logged-in session of any role while the
// read-only data push stays open to all. HTTP has no write routes yet, so
// requiredRole returns Role.None (everything open) — gate machine HTTP APIs
// here as you add them, e.g.:
//
//	if (ctx.Request.Method != "GET" && ctx.Request.Path.StartsWithSegments("/api/machine"))
//		return Role.Operator;
wsSrv.AuthorizeWrite = authn.LoggedIn;
Func<HttpContext, Role> requiredRole = _ => Role.None;
app.Use(async (ctx, next) =>
{
    if (await authn.Authorize(ctx, requiredRole))
        await next();
});

authn.RegisterRoutes(app); // /api/login, /api/logout, /api/auth/status — never gated
app.MapGet("/ws", wsSrv.Handle);

// Frontend static files. The Go bridge embeds frontend/dist at compile time;
// C# serves it from disk — -webroot, or auto-detected relative to the working
// directory / binary.
string root = webroot.Value;
if (root == "")
{
    foreach (string probe in new[]
             {
                 "frontend/dist",
                 Path.Combine(AppContext.BaseDirectory, "frontend/dist"),
                 "../frontend/dist",
                 "../../frontend/dist",
             })
    {
        if (Directory.Exists(probe))
        {
            root = probe;
            break;
        }
    }
}
if (root != "" && Directory.Exists(root))
{
    var provider = new PhysicalFileProvider(Path.GetFullPath(root));
    app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = provider });
    app.UseStaticFiles(new StaticFileOptions { FileProvider = provider });
    BridgeLog.Print($"serving frontend from {Path.GetFullPath(root)}");
}
else
{
    BridgeLog.Print("frontend dist not found — static UI disabled (set -webroot)");
}

if (usingDefault)
    BridgeLog.Print($"auth enabled with built-in DEFAULT password \"{DefaultVendorPassword}\" — set PLC_BRIDGE_PASSWORD_HASH to change it before production");
else if (authn.Enabled)
    BridgeLog.Print("auth enabled (vendor/tuner/operator hashes from env)");
else
    BridgeLog.Print("auth disabled — all routes open");

string proto = certFile != "" ? "https" : "http";
BridgeLog.Print($"{proto} listening on {httpAddr.Value}");
BridgeLog.Print($"plc_bridge running. modbus={modbusAddr.Value} {proto}={httpAddr.Value} poll={GoDuration.Format(pollInterval.Value)} push={GoDuration.Format(pushInterval.Value)}");

try
{
    await app.RunAsync(cts.Token);
}
catch (OperationCanceledException)
{
}
cts.Cancel();
await pollTask;
dataMap.Dispose();
cmdMap.Dispose();
return 0;
