// Modbus TCP slave. Implements function codes 0x03 (read holding),
// 0x06 (write single holding), 0x10 (write multiple holding).
// Anything else returns exception 01 (illegal function).
//
// The implementation is deliberately small and dependency-free —
// Modbus TCP framing is a 7-byte MBAP header plus a PDU, and we only
// need three function codes for plc_bridge's purposes.

using System.Buffers.Binary;
using System.Net;
using System.Net.Sockets;
using PlcBridge.CmdSink;
using PlcBridge.State;
using PlcBridge.Util;

namespace PlcBridge.Modbus;

public sealed class ModbusServer
{
    private const byte FnReadHolding = 0x03;
    private const byte FnWriteSingleHolding = 0x06;
    private const byte FnWriteMultipleHolding = 0x10;

    private const byte ExcIllegalFunction = 0x01;
    private const byte ExcIllegalDataAddress = 0x02;
    private const byte ExcIllegalDataValue = 0x03;
    private const byte ExcServerFailure = 0x04;

    public required Snapshot Snapshot { get; init; }
    public ICommandSink? Commands { get; init; }

    // Snapshot age past which reads fail with exception 04 (server failure)
    // instead of serving frozen values as live data. Defaults to 500ms.
    public TimeSpan StaleAfter { get; init; }

    private TimeSpan EffectiveStaleAfter =>
        StaleAfter > TimeSpan.Zero ? StaleAfter : TimeSpan.FromMilliseconds(500);

    public async Task ListenAndServe(string addr, CancellationToken ct)
    {
        var (host, port) = NetAddr.Parse(addr);
        var ln = new TcpListener(host ?? IPAddress.Any, port);
        ln.Start();
        BridgeLog.Print($"modbus tcp listening on {addr}");
        try
        {
            while (true)
            {
                TcpClient conn = await ln.AcceptTcpClientAsync(ct);
                _ = Task.Run(() => Handle(conn, ct), CancellationToken.None);
            }
        }
        finally
        {
            ln.Stop();
        }
    }

    private async Task Handle(TcpClient conn, CancellationToken ct)
    {
        using var _ = conn;
        NetworkStream stream;
        try
        {
            stream = conn.GetStream();
        }
        catch
        {
            return;
        }
        var mbap = new byte[7];
        try
        {
            while (true)
            {
                await stream.ReadExactlyAsync(mbap, ct);
                ushort txn = BinaryPrimitives.ReadUInt16BigEndian(mbap.AsSpan(0, 2));
                ushort proto = BinaryPrimitives.ReadUInt16BigEndian(mbap.AsSpan(2, 2));
                ushort length = BinaryPrimitives.ReadUInt16BigEndian(mbap.AsSpan(4, 2));
                byte unitId = mbap[6];

                // Modbus TCP: length covers UnitID (1 byte) + PDU.
                if (proto != 0 || length < 2 || length > 253)
                    return;
                var pdu = new byte[length - 1];
                await stream.ReadExactlyAsync(pdu, ct);

                byte[] resp = Dispatch(pdu);
                if (resp.Length == 0)
                    return;

                var outBuf = new byte[7 + resp.Length];
                BinaryPrimitives.WriteUInt16BigEndian(outBuf.AsSpan(0, 2), txn);
                BinaryPrimitives.WriteUInt16BigEndian(outBuf.AsSpan(2, 2), 0);
                BinaryPrimitives.WriteUInt16BigEndian(outBuf.AsSpan(4, 2), (ushort)(resp.Length + 1));
                outBuf[6] = unitId;
                resp.CopyTo(outBuf, 7);
                await stream.WriteAsync(outBuf, ct);
            }
        }
        catch
        {
            // Peer gone, short read, or shutdown — drop the connection.
        }
    }

    private byte[] Dispatch(byte[] pdu)
    {
        if (pdu.Length < 1)
            return [];
        return pdu[0] switch
        {
            FnReadHolding => HandleRead(pdu),
            FnWriteSingleHolding => HandleWriteSingle(pdu),
            FnWriteMultipleHolding => HandleWriteMultiple(pdu),
            _ => Exception(pdu[0], ExcIllegalFunction),
        };
    }

    private byte[] HandleRead(byte[] pdu)
    {
        if (pdu.Length != 5)
            return Exception(pdu[0], ExcIllegalDataValue);
        ushort addr = BinaryPrimitives.ReadUInt16BigEndian(pdu.AsSpan(1, 2));
        ushort qty = BinaryPrimitives.ReadUInt16BigEndian(pdu.AsSpan(3, 2));
        if (qty < 1 || qty > 125)
            return Exception(pdu[0], ExcIllegalDataValue);
        if (addr + qty > Addresses.HoldingMapSize)
            return Exception(pdu[0], ExcIllegalDataAddress);

        // Stale data is as dangerous as no data to a Modbus master polling for
        // live state — fail the read instead of serving frozen values.
        if (!Snapshot.Read(out var data, out var age) || age > EffectiveStaleAfter)
            return Exception(pdu[0], ExcServerFailure);

        Span<ushort> regs = stackalloc ushort[Addresses.HoldingMapSize];
        Addresses.EncodeData(in data, regs);

        var outBuf = new byte[2 + qty * 2];
        outBuf[0] = pdu[0];
        outBuf[1] = (byte)(qty * 2);
        for (int i = 0; i < qty; i++)
            BinaryPrimitives.WriteUInt16BigEndian(outBuf.AsSpan(2 + i * 2, 2), regs[addr + i]);
        return outBuf;
    }

    private byte[] HandleWriteSingle(byte[] pdu)
    {
        if (pdu.Length != 5)
            return Exception(pdu[0], ExcIllegalDataValue);
        ushort addr = BinaryPrimitives.ReadUInt16BigEndian(pdu.AsSpan(1, 2));
        ushort val = BinaryPrimitives.ReadUInt16BigEndian(pdu.AsSpan(3, 2));
        string? err = ApplyWrite(addr, 1, [val]);
        if (err != null)
        {
            BridgeLog.Print($"modbus write @{addr}: {err}");
            return Exception(pdu[0], ExcIllegalDataAddress);
        }
        return pdu[..5]; // echo per spec
    }

    private byte[] HandleWriteMultiple(byte[] pdu)
    {
        if (pdu.Length < 6)
            return Exception(pdu[0], ExcIllegalDataValue);
        ushort addr = BinaryPrimitives.ReadUInt16BigEndian(pdu.AsSpan(1, 2));
        ushort qty = BinaryPrimitives.ReadUInt16BigEndian(pdu.AsSpan(3, 2));
        byte bc = pdu[5];
        if (bc != qty * 2 || pdu.Length != 6 + bc)
            return Exception(pdu[0], ExcIllegalDataValue);
        var regs = new ushort[qty];
        for (int i = 0; i < qty; i++)
            regs[i] = BinaryPrimitives.ReadUInt16BigEndian(pdu.AsSpan(6 + i * 2, 2));
        string? err = ApplyWrite(addr, qty, regs);
        if (err != null)
        {
            BridgeLog.Print($"modbus write @{addr}..{addr + qty}: {err}");
            return Exception(pdu[0], ExcIllegalDataAddress);
        }
        var outBuf = new byte[5];
        outBuf[0] = pdu[0];
        BinaryPrimitives.WriteUInt16BigEndian(outBuf.AsSpan(1, 2), addr);
        BinaryPrimitives.WriteUInt16BigEndian(outBuf.AsSpan(3, 2), qty);
        return outBuf;
    }

    // Hands the decode work to ICommandSink.Apply, which runs the mutator
    // under its own lock. ApplyCommandWrite reports partial or unmapped
    // writes; the sink rolls back and we surface exception 02 to the master.
    private string? ApplyWrite(ushort addr, ushort qty, ushort[] regs)
    {
        if (Commands == null)
            return "command sink unavailable (PLC shm not mounted)";
        return Commands.Apply((ref PlcBridge.Shm.PlcCommand c) =>
            Addresses.ApplyCommandWrite(ref c, addr, qty, regs));
    }

    private static byte[] Exception(byte fn, byte code) => [(byte)(fn | 0x80), code];
}
