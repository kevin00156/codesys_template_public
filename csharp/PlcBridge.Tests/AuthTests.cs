// Port of backend/internal/auth/auth_test.go, on DefaultHttpContext instead
// of httptest. Authorize() returning true ≙ the Go test's "next handler
// reached".

using System.Text;
using Microsoft.AspNetCore.Http;
using PlcBridge.Auth;
using Xunit;

namespace PlcBridge.Tests;

public class AuthTests
{
    private static DefaultHttpContext Ctx(string method, string path, string? jsonBody = null, string? cookie = null)
    {
        var ctx = new DefaultHttpContext();
        ctx.Request.Method = method;
        ctx.Request.Path = path;
        ctx.Response.Body = new MemoryStream();
        if (jsonBody != null)
            ctx.Request.Body = new MemoryStream(Encoding.UTF8.GetBytes(jsonBody));
        if (cookie != null)
            ctx.Request.Headers.Cookie = cookie;
        return ctx;
    }

    private static Role VendorAll(HttpContext _) => Role.Vendor;

    // Mirrors a production requiredRole: vendor/tuner prefixes, and operator
    // on non-GET machine/orders/source.
    private static Role RouteMatrix(HttpContext r)
    {
        string p = r.Request.Path.Value ?? "";
        if (p.StartsWith("/api/ctdrive/"))
            return Role.Vendor;
        if (p.StartsWith("/api/driveshear/"))
            return Role.Tuner;
        if (r.Request.Method != "GET" &&
            (p.StartsWith("/api/machine/") || p.StartsWith("/api/orders") || p.StartsWith("/api/source")))
            return Role.Operator;
        return Role.None;
    }

    // Posts the password and returns the "name=value" session cookie (null on failure).
    private static async Task<string?> Login(Authenticator a, string pw)
    {
        var ctx = Ctx("POST", "/api/login", $"{{\"password\":\"{pw}\"}}");
        await a.HandleLogin(ctx);
        foreach (string? h in ctx.Response.Headers.SetCookie)
        {
            if (h != null && h.StartsWith(Authenticator.CookieName + "=") && !h.StartsWith(Authenticator.CookieName + "=;"))
                return h.Split(';')[0];
        }
        return null;
    }

    [Fact]
    public async Task DisabledPassesEverything()
    {
        var a = new Authenticator("", "", "", false);
        Assert.False(a.Enabled, "empty hashes should disable auth");
        var ctx = Ctx("GET", "/api/ctdrive/params");
        Assert.True(await a.Authorize(ctx, VendorAll), "disabled auth must pass through");
    }

    [Fact]
    public async Task ProtectedBlockedWithoutSession()
    {
        var a = new Authenticator(Authenticator.HashPassword("hunter2"), "", "", false);
        var ctx = Ctx("GET", "/api/ctdrive/params");
        Assert.False(await a.Authorize(ctx, VendorAll), "protected route reached without a session");
        Assert.Equal(StatusCodes.Status401Unauthorized, ctx.Response.StatusCode);
    }

    [Fact]
    public async Task UnprotectedAlwaysOpen()
    {
        var a = new Authenticator(Authenticator.HashPassword("hunter2"), "", "", false);
        var ctx = Ctx("GET", "/api/machine/state");
        Assert.True(await a.Authorize(ctx, RouteMatrix), "unprotected route must pass");
    }

    // With no operator hash configured, operator-tier writes stay open even
    // though auth (vendor) is enabled — existing two-tier deployments must not
    // break.
    [Fact]
    public async Task OperatorTierBackCompat()
    {
        var a = new Authenticator(Authenticator.HashPassword("vendorpw"), "", "", false);
        Assert.False(a.OperatorGated, "operator tier must be off without a hash");
        var ctx = Ctx("POST", "/api/machine/produce");
        Assert.True(await a.Authorize(ctx, RouteMatrix), "operator write must stay open without operator hash");
    }

    // With an operator hash set, operator-surface writes need a session (any
    // role), reads stay open, and the operator role does not unlock
    // tuner/vendor surfaces.
    [Fact]
    public async Task OperatorTier()
    {
        var a = new Authenticator(
            Authenticator.HashPassword("vendorpw"), "", Authenticator.HashPassword("operatorpw"), false);

        async Task<bool> Try(string? cookie, string method, string path) =>
            await a.Authorize(Ctx(method, path, cookie: cookie), RouteMatrix);

        // Not logged in: writes blocked, reads open.
        Assert.False(await Try(null, "POST", "/api/machine/produce"), "anon operator write");
        Assert.False(await Try(null, "POST", "/api/orders/start"), "anon orders write");
        Assert.True(await Try(null, "GET", "/api/machine/state"), "anon machine read");
        Assert.True(await Try(null, "GET", "/api/orders/progress"), "anon orders read");

        // Operator password: operator writes open, tuner/vendor surfaces stay shut.
        string? op = await Login(a, "operatorpw");
        Assert.NotNull(op);
        Assert.True(await Try(op, "POST", "/api/machine/produce"), "operator on machine write");
        Assert.True(await Try(op, "POST", "/api/source"), "operator on source write");
        Assert.False(await Try(op, "POST", "/api/driveshear/cmd"), "operator on tuner route");
        Assert.False(await Try(op, "GET", "/api/ctdrive/params"), "operator on vendor route");

        // Vendor satisfies operator.
        string? vendor = await Login(a, "vendorpw");
        Assert.NotNull(vendor);
        Assert.True(await Try(vendor, "POST", "/api/machine/produce"), "vendor on operator write");
    }

    [Fact]
    public async Task LoginGrantsAccess()
    {
        var a = new Authenticator(Authenticator.HashPassword("hunter2"), "", "", false);

        // Wrong password -> 401, no cookie.
        var rec = Ctx("POST", "/api/login", "{\"password\":\"nope\"}");
        await a.HandleLogin(rec);
        Assert.Equal(StatusCodes.Status401Unauthorized, rec.Response.StatusCode);
        Assert.Equal(0, rec.Response.Headers.SetCookie.Count);

        // Right password -> session cookie that unlocks a protected route.
        var loginCtx = Ctx("POST", "/api/login", "{\"password\":\"hunter2\"}");
        await a.HandleLogin(loginCtx);
        string? setCookie = loginCtx.Response.Headers.SetCookie.FirstOrDefault();
        Assert.NotNull(setCookie);
        Assert.Contains("httponly", setCookie!.ToLowerInvariant());
        string cookie = setCookie.Split(';')[0];

        var okCtx = Ctx("GET", "/api/ctdrive/params", cookie: cookie);
        Assert.True(await a.Authorize(okCtx, VendorAll), "valid session must pass");

        // Logout invalidates it.
        await a.HandleLogout(Ctx("POST", "/api/logout", cookie: cookie));

        var rejectedCtx = Ctx("GET", "/api/ctdrive/params", cookie: cookie);
        Assert.False(await a.Authorize(rejectedCtx, VendorAll), "logged-out cookie must be rejected");
        Assert.Equal(StatusCodes.Status401Unauthorized, rejectedCtx.Response.StatusCode);
    }

    // The tuner password opens tuner routes but not vendor routes; the vendor
    // password opens both.
    [Fact]
    public async Task TunerRoleMatrix()
    {
        var a = new Authenticator(
            Authenticator.HashPassword("vendorpw"), Authenticator.HashPassword("tunerpw"), "", false);

        async Task<bool> Try(string? cookie, string path) =>
            await a.Authorize(Ctx("GET", path, cookie: cookie), RouteMatrix);

        string? tuner = await Login(a, "tunerpw");
        Assert.NotNull(tuner);
        Assert.True(await Try(tuner, "/api/driveshear/cmd"), "tuner on tuner route");
        Assert.False(await Try(tuner, "/api/ctdrive/params"), "tuner on vendor route");

        string? vendor = await Login(a, "vendorpw");
        Assert.NotNull(vendor);
        Assert.True(await Try(vendor, "/api/driveshear/cmd"), "vendor on tuner route");
        Assert.True(await Try(vendor, "/api/ctdrive/params"), "vendor on vendor route");
    }

    // The write gate used by the WebSocket command path. With auth disabled
    // every request may write; with auth enabled only a request carrying a
    // valid session of any role may.
    [Fact]
    public async Task LoggedIn()
    {
        // Disabled: everyone may write.
        Assert.True(new Authenticator("", "", "", false).LoggedIn(Ctx("GET", "/ws")));

        var a = new Authenticator(Authenticator.HashPassword("hunter2"), "", "", false);

        // No cookie: blocked.
        Assert.False(a.LoggedIn(Ctx("GET", "/ws")));

        // Valid session: allowed.
        string? cookie = await Login(a, "hunter2");
        Assert.NotNull(cookie);
        Assert.True(a.LoggedIn(Ctx("GET", "/ws", cookie: cookie)));
    }

    [Fact]
    public async Task SecureFlagFollowsTLS()
    {
        var a = new Authenticator(Authenticator.HashPassword("x"), "", "", true); // serving TLS
        var ctx = Ctx("POST", "/api/login", "{\"password\":\"x\"}");
        await a.HandleLogin(ctx);
        foreach (string? c in ctx.Response.Headers.SetCookie)
        {
            if (c != null && c.StartsWith(Authenticator.CookieName + "="))
                Assert.Contains("secure", c.ToLowerInvariant());
        }
    }
}
