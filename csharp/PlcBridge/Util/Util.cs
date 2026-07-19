using System.Globalization;
using System.Net;

namespace PlcBridge.Util;

/// <summary>Plain stderr-free console logging in Go's LstdFlags|Lmicroseconds format.</summary>
public static class BridgeLog
{
    public static void Print(string msg) =>
        Console.WriteLine($"{DateTime.Now:yyyy/MM/dd HH:mm:ss.ffffff} {msg}");

    /// <summary>log.Fatalf counterpart: print and exit(1).</summary>
    public static void Fatal(string msg)
    {
        Print(msg);
        Environment.Exit(1);
    }
}

/// <summary>
/// Go time.Duration string syntax ("10ms", "1.5s", "1m30s"), so the CLI flags
/// and docs stay identical across the Go and C# bridges.
/// </summary>
public static class GoDuration
{
    public static TimeSpan Parse(string s)
    {
        if (string.IsNullOrEmpty(s))
            throw new FormatException($"invalid duration \"{s}\"");
        double totalNs = 0;
        int i = 0;
        bool neg = false;
        if (s[i] is '+' or '-')
        {
            neg = s[i] == '-';
            i++;
        }
        // Go's ParseDuration special case: a bare "0" (or "+0"/"-0") needs no
        // unit. `-stale-after 0` from a Go-era unit file must keep working.
        if (s[i..] == "0")
            return TimeSpan.Zero;
        if (i == s.Length)
            throw new FormatException($"invalid duration \"{s}\"");
        while (i < s.Length)
        {
            int start = i;
            while (i < s.Length && (char.IsAsciiDigit(s[i]) || s[i] == '.'))
                i++;
            if (i == start)
                throw new FormatException($"invalid duration \"{s}\"");
            double v = double.Parse(s[start..i], CultureInfo.InvariantCulture);

            int unitStart = i;
            while (i < s.Length && !char.IsAsciiDigit(s[i]) && s[i] != '.')
                i++;
            double unitNs = s[unitStart..i] switch
            {
                "ns" => 1,
                "us" or "µs" or "μs" => 1e3,
                "ms" => 1e6,
                "s" => 1e9,
                "m" => 60e9,
                "h" => 3600e9,
                _ => throw new FormatException($"unknown unit in duration \"{s}\""),
            };
            totalNs += v * unitNs;
        }
        return TimeSpan.FromTicks((long)(totalNs / 100) * (neg ? -1 : 1));
    }

    /// <summary>Compact Go-style rendering for log lines (10ms, 1.5s, 2m).</summary>
    public static string Format(TimeSpan t)
    {
        if (t == TimeSpan.Zero)
            return "0s";
        double ms = t.TotalMilliseconds;
        if (ms < 1000)
            return ms == Math.Floor(ms) ? $"{(long)ms}ms" : $"{ms.ToString(CultureInfo.InvariantCulture)}ms";
        double sec = t.TotalSeconds;
        return sec == Math.Floor(sec) ? $"{(long)sec}s" : $"{sec.ToString(CultureInfo.InvariantCulture)}s";
    }
}

/// <summary>Go net-style "host:port" listen addresses (":8443", "127.0.0.1:5020", "localhost:8443").</summary>
public static class NetAddr
{
    public static (IPAddress? Host, int Port) Parse(string addr)
    {
        int i = addr.LastIndexOf(':');
        if (i < 0 || !int.TryParse(addr[(i + 1)..], out int port))
            throw new FormatException($"invalid listen address \"{addr}\"");
        string host = addr[..i].Trim('[', ']');
        if (host.Length == 0)
            return (null, port);
        if (IPAddress.TryParse(host, out IPAddress? ip))
            return (ip, port);
        // Go's net.Listen resolves hostnames ("localhost:8443"); do the same
        // rather than crashing on a Go-era unit file. Prefer IPv4 to match the
        // dial expectations of the existing tooling.
        IPAddress[] resolved;
        try
        {
            resolved = Dns.GetHostAddresses(host);
        }
        catch (Exception e)
        {
            throw new FormatException($"listen {addr}: {e.Message}");
        }
        if (resolved.Length == 0)
            throw new FormatException($"listen {addr}: no addresses for {host}");
        IPAddress pick = resolved.FirstOrDefault(a => a.AddressFamily == System.Net.Sockets.AddressFamily.InterNetwork)
                         ?? resolved[0];
        return (pick, port);
    }
}
