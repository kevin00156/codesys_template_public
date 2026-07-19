// Gates routes behind three role passwords — the C# counterpart of
// backend/internal/auth/auth.go:
//
//   - operator (操作員): may command the routine operator surfaces (machine
//     start/stop and the like). Opt-in: with no operator password configured
//     those surfaces stay open, preserving simpler two-tier deployments.
//   - tuner  (調機): everything operator has, plus the tuning / debug surfaces
//     — but not the configuration ones.
//   - vendor (廠商): full access — everything tuner has, plus parameters and
//     machine configuration.
//
// The data model stays deliberately small: no user accounts, just one bcrypt
// hash per role and a set of live session tokens (token → role) in memory. A
// process restart drops every session — an HMI has no durable-session
// requirement, operators simply log in again. Hashes come from environment
// variables (PLC_BRIDGE_PASSWORD_HASH = vendor, kept for back-compat;
// PLC_BRIDGE_TUNER_HASH = tuner; PLC_BRIDGE_OPERATOR_HASH = operator) so they
// never land in process args, shell history, or git — and they are the very
// same bcrypt hashes the Go bridge uses: swapping implementations never
// invalidates a deployed password. Login is a single password field: it is
// matched against vendor first, then tuner, then operator — the password
// decides the role.
//
// Enforcement is one chokepoint: Authorize() guards every request via a
// caller-supplied requiredRole(ctx) predicate. Hiding tabs in the frontend is
// cosmetic; this is the wall.
//
// Endpoints (registered unprotected, so the login screen can reach them):
//
//	POST /api/login        {password}  -> sets an HttpOnly session cookie
//	POST /api/logout                   -> clears it
//	GET  /api/auth/status              -> {enabled, loggedIn, role}

using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace PlcBridge.Auth;

/// <summary>Session privilege level, strictly ordered: vendor ⊇ tuner ⊇ operator.</summary>
public enum Role
{
    None,     // not logged in / open route
    Operator, // 操作員
    Tuner,    // 調機
    Vendor,   // 廠商 (full access)
}

public static class RoleExtensions
{
    /// <summary>The wire spelling used in JSON bodies ("" for None).</summary>
    public static string Wire(this Role r) => r switch
    {
        Role.Operator => "operator",
        Role.Tuner => "tuner",
        Role.Vendor => "vendor",
        _ => "",
    };

    /// <summary>Reports whether a session role meets a route requirement.</summary>
    public static bool Satisfies(this Role r, Role required) => required switch
    {
        Role.None => true,
        Role.Operator => r != Role.None, // any logged-in role may operate
        Role.Tuner => r is Role.Tuner or Role.Vendor,
        _ => r == Role.Vendor,
    };
}

public sealed class Authenticator
{
    public const string CookieName = "plc_session";
    private static readonly TimeSpan SessionTtl = TimeSpan.FromHours(12);

    private readonly string? _vendorHash;   // bcrypt; all three null => auth disabled
    private readonly string? _tunerHash;
    private readonly string? _operatorHash; // null => operator-tier routes stay open (back-compat)
    private readonly bool _secure;          // mark the session cookie Secure (HTTPS only)

    // Plaintext of the built-in template default login, set only when the
    // active vendor hash came from that default (no env hash configured).
    // Non-empty => the UI shows it on the login screen and nags the operator
    // to change it; surfaced via /api/auth/status.
    private string _defaultPassword = "";

    private readonly object _mu = new();
    private readonly Dictionary<string, Session> _tokens = [];

    private readonly record struct Session(Role Role, DateTimeOffset Exp);

    private static readonly JsonSerializerOptions JsonOpts = new()
    {
        PropertyNameCaseInsensitive = true,
    };

    /// <summary>
    /// Empty hashes disable auth entirely: every route is open and
    /// /api/auth/status reports enabled:false — the intended posture on a dev
    /// box. secure should be true when serving TLS so the cookie is never sent
    /// in clear.
    /// </summary>
    public Authenticator(string? vendorHash, string? tunerHash, string? operatorHash, bool secure)
    {
        _vendorHash = string.IsNullOrEmpty(vendorHash) ? null : vendorHash;
        _tunerHash = string.IsNullOrEmpty(tunerHash) ? null : tunerHash;
        _operatorHash = string.IsNullOrEmpty(operatorHash) ? null : operatorHash;
        _secure = secure;
    }

    /// <summary>Whether any password is configured. When false everything passes.</summary>
    public bool Enabled => _vendorHash != null || _tunerHash != null || _operatorHash != null;

    /// <summary>
    /// Whether the operator tier is active. Routes requiring Role.Operator
    /// stay open while it is false, so deployments that only ever configured
    /// the vendor/tuner passwords keep their HMI operator surfaces working
    /// exactly as before.
    /// </summary>
    public bool OperatorGated => _operatorHash != null;

    /// <summary>
    /// Whether the request carries a valid session of any role. Used to gate
    /// write surfaces (the WebSocket command path) while leaving read-only
    /// telemetry open to everyone. With auth disabled it is always true.
    /// </summary>
    public bool LoggedIn(HttpContext ctx) => !Enabled || SessionRole(ctx) != Role.None;

    /// <summary>
    /// Records that the active vendor hash is the built-in template default
    /// with the given plaintext, so /api/auth/status can tell the UI to
    /// display it and nag the operator to change it.
    /// </summary>
    public void UseDefaultPassword(string plaintext) => _defaultPassword = plaintext;

    /// <summary>bcrypt hash suitable for the *_HASH env vars (plc_bridge -gen-hash).</summary>
    public static string HashPassword(string pw)
    {
        // Go's bcrypt refuses inputs past bcrypt's 72-byte limit instead of
        // silently truncating; BCrypt.Net does not check, so enforce the same
        // contract here — both CLIs must agree on which passwords are mintable.
        if (Encoding.UTF8.GetByteCount(pw) > 72)
            throw new ArgumentException("bcrypt: password length exceeds 72 bytes");
        return BCrypt.Net.BCrypt.HashPassword(pw, workFactor: 10); // Go bcrypt.DefaultCost
    }

    private static bool VerifyHash(string? hash, string pw)
    {
        if (hash == null)
            return false;
        try
        {
            return BCrypt.Net.BCrypt.Verify(pw, hash);
        }
        catch
        {
            return false; // malformed hash behaves like a mismatch, as in Go
        }
    }

    /// <summary>Mounts the login/logout/status endpoints. These must never be
    /// gated by Authorize's predicate, or the login screen can't reach them.</summary>
    public void RegisterRoutes(IEndpointRouteBuilder app)
    {
        app.MapPost("/api/login", HandleLogin);
        app.MapPost("/api/logout", HandleLogout);
        // Go 1.22's "GET /path" patterns also match HEAD; MapGet does not, and
        // a health probe using HEAD must not see 405 from one bridge only.
        app.MapMethods("/api/auth/status", ["GET", "HEAD"], HandleStatus);
    }

    /// <summary>
    /// The Wrap counterpart, shaped as middleware: returns true to continue
    /// down the pipeline, false after writing a 401. The predicate sees the
    /// whole request so it can gate by method as well as path (operator
    /// surfaces gate writes only — reads feed the always-on dashboard).
    /// </summary>
    public async Task<bool> Authorize(HttpContext ctx, Func<HttpContext, Role> requiredRole)
    {
        if (!Enabled)
            return true;
        Role req = requiredRole(ctx);
        if (req == Role.Operator && !OperatorGated)
            req = Role.None; // operator tier not configured — stays open
        if (req != Role.None && !SessionRole(ctx).Satisfies(req))
        {
            await WriteJson(ctx, StatusCodes.Status401Unauthorized,
                new Dictionary<string, object?> { ["error"] = "需要登入" });
            return false;
        }
        return true;
    }

    internal async Task HandleStatus(HttpContext ctx)
    {
        Role role = SessionRole(ctx);
        await WriteJson(ctx, StatusCodes.Status200OK, new Dictionary<string, object?>
        {
            ["enabled"] = Enabled,
            ["loggedIn"] = role != Role.None || !Enabled,
            ["role"] = role.Wire(),
            ["operatorGated"] = OperatorGated, // mirror for the frontend's tab locks
            // usingDefault => still on the built-in template password; defaultPassword
            // carries its plaintext so the login screen can show it. Empty unless the
            // default is active (a configured password is never echoed back).
            ["usingDefault"] = _defaultPassword != "",
            ["defaultPassword"] = _defaultPassword,
        });
    }

    private sealed class LoginBody
    {
        [JsonPropertyName("password")] public string? Password { get; set; }
    }

    internal async Task HandleLogin(HttpContext ctx)
    {
        if (!Enabled)
        {
            // Nothing to authenticate against — report success so the UI proceeds.
            await WriteJson(ctx, StatusCodes.Status200OK,
                new Dictionary<string, object?> { ["ok"] = true, ["role"] = Role.Vendor.Wire() });
            return;
        }
        // Go-decoder semantics: json.Decoder.Decode consumes exactly ONE JSON
        // value (bytes after it are never looked at) and unmarshals the
        // literal `null` as a no-op — an empty password that fails with 401,
        // not a 400. Deserializing the whole stream would reject both.
        LoginBody? body;
        try
        {
            byte[] raw = await ReadAll(ctx.Request.Body);
            body = ParseFirstJsonValue(raw) ?? new LoginBody();
        }
        catch (JsonException)
        {
            await WriteJson(ctx, StatusCodes.Status400BadRequest,
                new Dictionary<string, object?> { ["error"] = "bad json" });
            return;
        }
        string pw = body.Password ?? "";
        // The password decides the role: vendor, then tuner, then operator.
        // bcrypt's compare is constant-time per hash; all mismatching lands in
        // one generic error.
        Role role =
            VerifyHash(_vendorHash, pw) ? Role.Vendor :
            VerifyHash(_tunerHash, pw) ? Role.Tuner :
            VerifyHash(_operatorHash, pw) ? Role.Operator : Role.None;
        if (role == Role.None)
        {
            await WriteJson(ctx, StatusCodes.Status401Unauthorized,
                new Dictionary<string, object?> { ["error"] = "密碼錯誤" });
            return;
        }
        ctx.Response.Cookies.Append(CookieName, NewToken(role), new CookieOptions
        {
            Path = "/",
            HttpOnly = true,
            Secure = _secure,
            SameSite = SameSiteMode.Strict,
            MaxAge = SessionTtl,
        });
        await WriteJson(ctx, StatusCodes.Status200OK,
            new Dictionary<string, object?> { ["ok"] = true, ["role"] = role.Wire() });
    }

    internal async Task HandleLogout(HttpContext ctx)
    {
        if (ctx.Request.Cookies.TryGetValue(CookieName, out string? tok))
        {
            lock (_mu)
                _tokens.Remove(tok);
        }
        ctx.Response.Cookies.Delete(CookieName, new CookieOptions
        {
            Path = "/",
            HttpOnly = true,
            Secure = _secure,
            SameSite = SameSiteMode.Strict,
        });
        await WriteJson(ctx, StatusCodes.Status200OK,
            new Dictionary<string, object?> { ["ok"] = true });
    }

    /// <summary>
    /// Mints a 256-bit random session token, records it with its role, and
    /// opportunistically garbage-collects expired tokens.
    /// </summary>
    private string NewToken(Role role)
    {
        Span<byte> b = stackalloc byte[32];
        RandomNumberGenerator.Fill(b);
        string tok = Convert.ToHexString(b).ToLowerInvariant();

        DateTimeOffset now = DateTimeOffset.UtcNow;
        lock (_mu)
        {
            foreach (string k in _tokens.Where(kv => now > kv.Value.Exp).Select(kv => kv.Key).ToList())
                _tokens.Remove(k);
            _tokens[tok] = new Session(role, now + SessionTtl);
        }
        return tok;
    }

    /// <summary>
    /// The live session's role, or None. Auth disabled means everyone is
    /// effectively vendor (everything open). A 256-bit random token makes the
    /// dictionary lookup safe without constant-time comparison.
    /// </summary>
    private Role SessionRole(HttpContext ctx)
    {
        if (!Enabled)
            return Role.Vendor;
        if (!ctx.Request.Cookies.TryGetValue(CookieName, out string? tok))
            return Role.None;
        lock (_mu)
        {
            if (!_tokens.TryGetValue(tok, out Session s))
                return Role.None;
            if (DateTimeOffset.UtcNow > s.Exp)
            {
                _tokens.Remove(tok);
                return Role.None;
            }
            return s.Role;
        }
    }

    // Utf8JsonReader is a ref struct and cannot live in an async method.
    private static LoginBody? ParseFirstJsonValue(byte[] raw)
    {
        var reader = new Utf8JsonReader(raw);
        return JsonSerializer.Deserialize<LoginBody>(ref reader, JsonOpts);
    }

    private static async Task<byte[]> ReadAll(Stream s)
    {
        using var ms = new MemoryStream();
        await s.CopyToAsync(ms);
        return ms.ToArray();
    }

    private static async Task WriteJson(HttpContext ctx, int status, Dictionary<string, object?> body)
    {
        ctx.Response.StatusCode = status;
        ctx.Response.ContentType = "application/json; charset=utf-8";
        ctx.Response.Headers.CacheControl = "no-store";
        if (HttpMethods.IsHead(ctx.Request.Method))
            return; // headers + status only, like Go's ResponseWriter on HEAD
        await JsonSerializer.SerializeAsync(ctx.Response.Body, body);
    }
}
