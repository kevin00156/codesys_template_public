// A minimal Go-flag-style command-line parser, so the C# bridge's CLI is
// identical to the Go bridge's: -name value, -name=value, --name, and bare
// boolean flags. Unknown flags print usage and exit 2 (Go's flag package
// behaviour). Durations use Go syntax ("10ms", "1.5s").

using System.Text;

namespace PlcBridge.Util;

public sealed class Flag<T>
{
    public T Value { get; internal set; } = default!;
}

public sealed class Flags(string program)
{
    private sealed record Entry(string Name, string Help, string Default, bool IsBool, Action<string> Set);

    private readonly List<Entry> _entries = [];

    public Flag<string> String(string name, string def, string help)
    {
        var f = new Flag<string> { Value = def };
        _entries.Add(new Entry(name, help, $"\"{def}\"", false, v => f.Value = v));
        return f;
    }

    public Flag<TimeSpan> Duration(string name, TimeSpan def, string help)
    {
        var f = new Flag<TimeSpan> { Value = def };
        _entries.Add(new Entry(name, help, GoDuration.Format(def), false, v => f.Value = GoDuration.Parse(v)));
        return f;
    }

    public Flag<bool> Bool(string name, bool def, string help)
    {
        var f = new Flag<bool> { Value = def };
        _entries.Add(new Entry(name, help, def ? "true" : "false", true, v => f.Value = ParseBool(v)));
        return f;
    }

    // Go's strconv.ParseBool: 1/t/T/TRUE/true/True and 0/f/F/FALSE/false/False.
    private static bool ParseBool(string v) => v switch
    {
        "1" or "t" or "T" or "TRUE" or "true" or "True" => true,
        "0" or "f" or "F" or "FALSE" or "false" or "False" => false,
        _ => throw new FormatException($"invalid boolean value \"{v}\""),
    };

    public void Parse(string[] args)
    {
        int i = 0;
        while (i < args.Length)
        {
            string arg = args[i];
            if (!arg.StartsWith('-'))
                break; // Go's flag package stops at the first non-flag argument
            string name = arg.TrimStart('-');
            if (name is "h" or "help")
            {
                Console.Error.Write(Usage());
                Environment.Exit(0); // Go: -h prints usage and exits 0
            }
            string? inline = null;
            int eq = name.IndexOf('=');
            if (eq >= 0)
            {
                inline = name[(eq + 1)..];
                name = name[..eq];
            }
            Entry? e = _entries.FirstOrDefault(x => x.Name == name);
            if (e == null)
                Die($"flag provided but not defined: -{name}");
            string value;
            if (inline != null)
            {
                value = inline;
            }
            else if (e!.IsBool)
            {
                value = "true"; // bare boolean flag
            }
            else
            {
                i++;
                if (i >= args.Length)
                    Die($"flag needs an argument: -{name}");
                value = args[i];
            }
            try
            {
                e!.Set(value);
            }
            catch (Exception ex)
            {
                Die($"invalid value \"{value}\" for flag -{name}: {ex.Message}");
            }
            i++;
        }
    }

    private void Die(string msg)
    {
        Console.Error.WriteLine(msg);
        Console.Error.Write(Usage());
        Environment.Exit(2);
    }

    private string Usage()
    {
        var sb = new StringBuilder();
        sb.AppendLine($"Usage of {program}:");
        foreach (Entry e in _entries)
        {
            sb.AppendLine($"  -{e.Name}");
            sb.AppendLine($"    \t{e.Help} (default {e.Default})");
        }
        return sb.ToString();
    }
}
