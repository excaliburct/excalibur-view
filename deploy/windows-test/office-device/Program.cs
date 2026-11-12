// WHAT: two estimating computers linking to the Excalibur Office Server on this runner with LatCon Estimating 0.8.0's own
//       client code: the first asks and becomes the office's admin at once and sets the office up with one project; the second
//       asks and waits, the cell refuses it while it waits, the first approves it, and then the second reaches the cell's
//       store (the cell asking the Office Server about it) and the project.
// WHY:  part (d) of deploy/windows-test/office-e2e.ps1: the codeless linking, the device check across the pipe, and a
//       project, proved on a real Windows server with the real programs.
using System.Net;
using System.Net.Http.Headers;
using System.Runtime.Versioning;
using System.Text;
using System.Text.Json;
using Citadel.Client;
using Citadel.Core;
using Citadel.Net;

[assembly: SupportedOSPlatform("windows")] // WHAT: Windows only. WHY: the device key is kept with DPAPI, as LatCon keeps it

namespace OfficeDevice;

internal static class Program
{
    private const string Host = "127.0.0.1"; // WHAT: the Office Server on this runner. WHY: a local address, so a first contact may pin
    private const int OfficePort = 8714; // WHAT: the Office Server's port. WHY: hyperview_server::config::DEFAULT_PORT
    private const int CellPort = 47110; // WHAT: the Citadel cell's port. WHY: Wire.DefaultPort, unchanged for LatCon
    private static readonly TimeSpan CellPatience = TimeSpan.FromSeconds(90); // WHAT: how long the cell's answer is waited for. WHY: it keeps a refusal 60 s

    private static int Main(string[] args)
    {
        if (args is not ["two-computers", var work]) // WHAT: the one command. WHY: the script runs exactly this
        {
            Console.Error.WriteLine("usage: OfficeDevice two-computers <folder>"); // WHAT: how to use it. WHY: said plainly
            return 2; // WHAT: not understood. WHY: told apart from a failed check
        }

        try
        {
            TwoComputers(Path.GetFullPath(work)); // WHAT: the whole of part (d). WHY: below
            return 0; // WHAT: every check passed. WHY: the script reads it
        }
        catch (Exception e)
        {
            Console.Error.WriteLine("FAILED: " + e); // WHAT: what failed, with where. WHY: the run's log says why
            return 1; // WHAT: failed. WHY: the script stops
        }
    }

    // One computer: its own folder, its own DPAPI-sealed key and its own shared pin file, as on a real PC.
    private sealed record Computer(string Folder, DpapiDeviceKeyStore Keys, OfficeLinking Linking);

    private static Computer Make(string folder, string name)
    {
        Directory.CreateDirectory(folder); // WHAT: the computer's folder. WHY: its link, key and pin file live there
        var keys = new DpapiDeviceKeyStore(folder); // WHAT: the device key, sealed for this Windows account. WHY: LatCon's own store
        var who = new LinkWho(name, Environment.UserName, name, "0.8.0", "windows-x64"); // WHAT: who it says it is. WHY: the admin's list shows it
        var linking = new OfficeLinking(folder, keys, Path.Combine(folder, "office.json"), (h, p, pin) => new OfficeServerClient(h, p, pin), OfficeLinking.CellHello, () => DateTimeOffset.UtcNow, who); // WHAT: "Link to the office server", as the program builds it. WHY: the real path, each computer with its own shared file
        return new Computer(folder, keys, linking); // WHAT: ready. WHY: used below
    }

    private static void TwoComputers(string work)
    {
        var first = Make(Path.Combine(work, "first"), "E2E-ADMIN"); // WHAT: the first computer. WHY: it claims the office
        var second = Make(Path.Combine(work, "second"), "E2E-ESTIMATOR"); // WHAT: the second. WHY: it waits for approval

        var claimed = first.Linking.Ask(Host, OfficePort); // WHAT: the first computer asks. WHY: nobody has claimed the office yet
        Check(claimed.State == LinkState.Approved && claimed.Admin, "the first computer to ask is approved at once, as the office's admin"); // WHAT: claimed. WHY: §3
        var pin = first.Linking.Shared?.Pin ?? throw new InvalidDataException("The first computer kept no pin."); // WHAT: the Office Server's pin, kept on first contact. WHY: every call after holds to it
        var firstId = first.Linking.Pending?.OfficeDeviceId ?? throw new InvalidDataException("The first computer's request wasn't kept."); // WHAT: its office device id. WHY: it signs with it

        var password = "E2e-" + Guid.NewGuid().ToString("N"); // WHAT: a fresh password. WHY: a test account, nobody's real one
        const string email = "admin@example.com"; // WHAT: the admin's address. WHY: a reserved example domain
        var setup = Json(Signed(first, pin, firstId, HttpMethod.Post, "/api/v1/setup", new { company = "E2E Test Office", name = "E2E Admin", email, password }, null)); // WHAT: the office set up from the admin computer. WHY: the first person, as the admin's app does it
        var token = setup.GetProperty("session").GetProperty("token").GetString() ?? throw new InvalidDataException("No session came back."); // WHAT: the admin's session. WHY: projects need a person
        var project = Json(Signed(first, pin, firstId, HttpMethod.Post, "/api/v1/projects", new { number = "E2E-1", name = "End to end" }, token)); // WHAT: one project. WHY: the second computer must reach it
        var projectId = project.GetProperty("id").GetString() ?? throw new InvalidDataException("The project has no id."); // WHAT: its id. WHY: asked for by the second computer
        Console.WriteLine($"ok   the admin computer set the office up and made project E2E-1 ({projectId})"); // WHAT: said. WHY: the log reads as a list of what was proved

        var waiting = second.Linking.Ask(Host, OfficePort); // WHAT: the second computer asks. WHY: the office has an admin now
        Check(waiting.State == LinkState.Waiting, "the second computer waits for an admin"); // WHAT: waiting. WHY: §3
        var secondId = second.Linking.Pending?.OfficeDeviceId ?? throw new InvalidDataException("The second computer's request wasn't kept."); // WHAT: its office id. WHY: the admin approves it by this

        var key = second.Keys.Load() ?? throw new InvalidDataException("The second computer has no key."); // WHAT: its key. WHY: the cell's requests are signed with it
        try
        {
            var (hello, cellPin) = OfficeLinking.CellHello(Host, CellPort, null); // WHAT: the cell's hello and pin, on first contact. WHY: to ask the cell before approval
            var early = new ServerLink { Host = Host, Port = CellPort, ServerName = hello.Server, Spki = cellPin, DeviceId = DeviceKeys.IdOf(DeviceKeys.PublicKeyOf(key)), Connected = AuditLog.Time(DateTimeOffset.UtcNow), OfficeDeviceId = secondId, OfficePort = OfficePort, OfficePin = pin }; // WHAT: the link it would have. WHY: the cell is asked before it is approved
            Check(Refused(() => { using var cell = new OfficeClient(early, key); cell.List("", null); }), "the cell refuses the second computer while it waits"); // WHAT: refused. WHY: the cell asks the Office Server, which says waiting
        }
        finally
        {
            OfficeServerClient.Forget(key); // WHAT: the key's copy cleared. WHY: as LatCon does
        }

        var adminKey = first.Keys.Load() ?? throw new InvalidDataException("The admin computer has no key."); // WHAT: the admin's key. WHY: the list and the approval are signed
        try
        {
            using var office = new OfficeServerClient(Host, OfficePort, pin); // WHAT: the Office Server, pinned. WHY: §7 rule 5
            var listed = office.List(adminKey, firstId); // WHAT: the admin's Computers list. WHY: the second computer must be on it
            Check(listed.Any(d => d.DeviceId == secondId && d.State == OfficeDeviceState.Waiting), "the admin's list shows the second computer waiting"); // WHAT: listed. WHY: §3
            office.Approve(adminKey, firstId, secondId, admin: false); // WHAT: approved, not as an admin. WHY: an estimator's computer
        }
        finally
        {
            OfficeServerClient.Forget(adminKey); // WHAT: cleared. WHY: as above
        }

        var approved = second.Linking.Check(); // WHAT: the second computer asks how it stands. WHY: the waiting screen's 5 s look
        Check(approved.State == LinkState.Approved && !approved.Admin, "the second computer sees it is approved"); // WHAT: approved. WHY: §3
        var link = second.Linking.Finish(approved.CitadelPin); // WHAT: linked to the cell, as the program finishes. WHY: the cell's pin from the Office Server, or a first contact
        var records = -1; // WHAT: how many records the cell listed. WHY: said below
        key = second.Keys.Load() ?? throw new InvalidDataException("The second computer's key is gone."); // WHAT: its key again. WHY: cleared above
        try
        {
            var until = DateTime.UtcNow + CellPatience; // WHAT: the limit. WHY: the cell keeps the waiting answer a minute
            while (records < 0)
            {
                try
                {
                    using var cell = new OfficeClient(link, key); // WHAT: the cell's store, as the program opens it. WHY: every request signed with citadel-request/1 and X-Excalibur-Device
                    records = cell.List("", null).Records.Count; // WHAT: the store's list. WHY: the cell checks the device with the Office Server first
                }
                catch (Exception e) when (DateTime.UtcNow < until && e is not OutOfMemoryException)
                {
                    Console.WriteLine($"     the cell said: {e.Message} (asked again in 5 s)"); // WHAT: said. WHY: the log shows the wait
                    Thread.Sleep(TimeSpan.FromSeconds(5)); // WHAT: a pause. WHY: as the program's own pace
                }
            }
        }
        finally
        {
            OfficeServerClient.Forget(key); // WHAT: cleared. WHY: as above
        }

        Check(records >= 0, $"the cell answers the second computer: it asked the Office Server, which said approved ({records} records listed)"); // WHAT: the device check across the pipe. WHY: (d)
        var session = Json(Signed(second, pin, secondId, HttpMethod.Post, "/api/v1/auth/token", new { email, password }, null)); // WHAT: the admin signs in on the second computer. WHY: a project needs a person
        var secondToken = session.GetProperty("token").GetString() ?? throw new InvalidDataException("No session came back."); // WHAT: the session. WHY: below
        var reached = Json(Signed(second, pin, secondId, HttpMethod.Get, $"/api/v1/projects/{projectId}", null, secondToken)); // WHAT: the project, from the second computer. WHY: (d)
        Check(reached.GetProperty("number").GetString() == "E2E-1", "the second computer reaches project E2E-1"); // WHAT: the same project. WHY: (d)
    }

    // A request to the Office Server, signed with excalibur-request/1 by the computer's key, with a person's session when given.
    private static byte[] Signed(Computer computer, string pin, string deviceId, HttpMethod method, string target, object? body, string? token)
    {
        var key = computer.Keys.Load() ?? throw new InvalidDataException("This computer has no key."); // WHAT: the device key. WHY: it signs
        try
        {
            var bytes = body is null ? [] : JsonSerializer.SerializeToUtf8Bytes(body); // WHAT: the body as JSON. WHY: exactly the bytes hashed and signed
            var contact = new FirstContact(pin); // WHAT: held to the kept pin. WHY: §7 rule 5
            using var http = new HttpClient(contact.Handler(), disposeHandler: true) { Timeout = TimeSpan.FromSeconds(30) }; // WHAT: one client. WHY: one request
            using var request = new HttpRequestMessage(method, new Uri($"https://{Host}:{OfficePort}{target}")) { Version = HttpVersion.Version11 }; // WHAT: the request. WHY: HTTP/1.1, as LatCon sends it
            if (body is not null)
            {
                request.Content = new ByteArrayContent(bytes); // WHAT: the body. WHY: signed below
                request.Content.Headers.ContentType = new MediaTypeHeaderValue("application/json"); // WHAT: JSON. WHY: the server reads it so
            }

            OfficeSigner.Sign(key, pin, deviceId, method.Method, target, bytes, DateTimeOffset.UtcNow).ApplyTo(request.Headers); // WHAT: the four signed headers. WHY: §4, the gate
            if (token is not null)
            {
                request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token); // WHAT: the person's session. WHY: View's own sign-in check
            }

            using var response = http.Send(request); // WHAT: sent. WHY: one request
            var answer = response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult(); // WHAT: the answer. WHY: read below
            if (!response.IsSuccessStatusCode)
            {
                throw new InvalidDataException($"{method} {target}: {(int)response.StatusCode} {Encoding.UTF8.GetString(answer)}"); // WHAT: refused, with the server's words. WHY: the log says why
            }

            return answer; // WHAT: the body. WHY: read by the caller
        }
        finally
        {
            OfficeServerClient.Forget(key); // WHAT: cleared. WHY: as LatCon does
        }
    }

    private static JsonElement Json(byte[] bytes) => JsonDocument.Parse(bytes).RootElement.Clone(); // WHAT: the answer as JSON. WHY: read by name

    // Whether a call is refused (throws), saying what was said.
    private static bool Refused(Action call)
    {
        try
        {
            call(); // WHAT: tried. WHY: it should fail
            return false; // WHAT: let through. WHY: the check fails
        }
        catch (Exception e) when (e is not OutOfMemoryException)
        {
            Console.WriteLine($"     refused as it should be: {e.Message}"); // WHAT: said. WHY: the log shows how
            return true; // WHAT: refused. WHY: the check passes
        }
    }

    private static void Check(bool ok, string what)
    {
        if (!ok)
        {
            throw new InvalidOperationException("FAILED: " + what); // WHAT: stop. WHY: nothing after a failed check means anything
        }

        Console.WriteLine("ok   " + what); // WHAT: said. WHY: the log reads as the list of what was proved
    }
}
