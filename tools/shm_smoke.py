#!/usr/bin/env python3
"""End-to-end smoke of the Rust motion daemon over real /dev/shm segments.

Plays the Go bridge's role: writes plc_cmd under the seqlock protocol with the
per-axis touch mask (Header.Flags), reads plc_data, and checks the daemon's
observable behaviour: enable, jog, command dead-man, MoveAbs, per-axis fresh
(no phantom re-dispatch), EMS, SIGTERM graceful shutdown, and inode reuse
across a daemon restart (the bridge's once-opened mmap keeps working).

Usage (Linux; sim backend, no hardware):
    cargo build -p motion-daemon
    python3 tools/shm_smoke.py rust/target/debug/motion-daemon [cycle_ms]

Without root, run it in a private /dev/shm so it never touches the machine's
real segments:
    unshare -Urm bash -c 'mount -t tmpfs tmpfs /dev/shm && \
        python3 tools/shm_smoke.py rust/target/debug/motion-daemon'

Exit code 0 = every check passed. This is the daemon-side counterpart of
backend/e2e_smoke (which simulates the PLC to test the bridge).
"""
import mmap, os, signal, struct, subprocess, sys, time

DAEMON = sys.argv[1]
CYCLE_MS = float(sys.argv[2]) if len(sys.argv) > 2 else 2.0

DATA_MAGIC, DATA_VER, DATA_SIZE = 0x504C4344, 4, 248
CMD_MAGIC, CMD_VER, CMD_SIZE = 0x504C4343, 3, 168
ENABLE, HOME, RESET, STOP, JOG_POS, JOG_NEG, MOVE_ABS = 1, 2, 4, 8, 16, 32, 64
MASK_PRESENT = 0x8000
ST_ENABLED, ST_BUSY, ST_ERROR, ST_STANDSTILL = 1, 2, 4, 8
ALARM_BUS_FAULT, ALARM_CMD_TIMEOUT = 1, 2
STEP = {0: "IDLE", 10: "ENABLING", 20: "READY", 30: "JOG_POS", 40: "MOVE_ABS", 90: "STOPPING", 91: "WAIT_STOP", 95: "TRY_RESET"}

failures = []

def check(cond, msg):
    print(("  ok   " if cond else "  FAIL ") + msg)
    if not cond:
        failures.append(msg)

def open_map(name, size, write):
    fd = os.open("/dev/shm/" + name, os.O_RDWR if write else os.O_RDONLY)
    m = mmap.mmap(fd, size, prot=(mmap.PROT_READ | mmap.PROT_WRITE) if write else mmap.PROT_READ)
    os.close(fd)
    return m

def read_data(m):
    for _ in range(100):
        s1 = struct.unpack_from("<I", m, 8)[0]
        if s1 & 1:
            continue
        buf = bytes(m[:DATA_SIZE])
        s2 = struct.unpack_from("<I", m, 8)[0]
        if s1 != s2:
            continue
        magic, ver, flags, seq, _pad, cycle = struct.unpack_from("<IHHIIQ", buf, 0)
        if magic != DATA_MAGIC or ver != DATA_VER:
            return None
        temp, status_flags, alarm_flags = struct.unpack_from("<dII", buf, 24)
        axes = []
        for i in range(4):
            off = 40 + i * 48
            ap, av, sp, sv, step, fl, err, _p = struct.unpack_from("<ddddiIii", buf, off)
            axes.append(dict(act_pos=ap, act_vel=av, set_pos=sp, set_vel=sv, step=step, flags=fl, error_id=err))
        return dict(cycle=cycle, status_flags=status_flags, alarm_flags=alarm_flags, axes=axes)
    return None

class CmdWriter:
    def __init__(self, m, cycle=0):
        self.m = m
        self.cycle = cycle
        self.axes = [dict(word=0, jog_vel=0.0, pos=0.0, vel=0.0) for _ in range(4)]
        self.machine = 0
        self.prod = 0

    def publish(self, touched):
        self.cycle += 1
        flags = MASK_PRESENT | touched
        s = struct.unpack_from("<I", self.m, 8)[0]
        odd = (s + 1 + (s & 1)) & 0xFFFFFFFF
        struct.pack_into("<I", self.m, 8, odd)
        buf = bytearray(CMD_SIZE)
        struct.pack_into("<IHHIIQ", buf, 0, CMD_MAGIC, CMD_VER, flags, odd, 0, self.cycle)
        for i, a in enumerate(self.axes):
            struct.pack_into("<IIddd", buf, 24 + i * 32, a["word"], 0, a["jog_vel"], a["pos"], a["vel"])
        struct.pack_into("<II", buf, 152, self.machine, 0)
        struct.pack_into("<ii", buf, 160, self.prod, 0)
        self.m[0:CMD_SIZE] = bytes(buf)
        struct.pack_into("<I", self.m, 8, (odd + 1) & 0xFFFFFFFF)

    def axis(self, i, word, jog_vel=0.0, pos=0.0, vel=0.0):
        self.axes[i] = dict(word=word, jog_vel=jog_vel, pos=pos, vel=vel)
        self.publish(1 << i)

    def touch_only(self, i):
        self.publish(1 << i)

def wait_for(pred, timeout, desc):
    t0 = time.time()
    last = None
    while time.time() - t0 < timeout:
        d = read_data(data)
        last = d
        if d is not None and pred(d):
            return d
        time.sleep(0.01)
    print(f"  (timeout waiting for {desc}; last = {summ(last)})")
    return last

def summ(d):
    if d is None:
        return "None"
    a0 = d["axes"][0]
    return f"cycle={d['cycle']} alarm={d['alarm_flags']} ax0 step={STEP.get(a0['step'], a0['step'])} flags={a0['flags']} pos={a0['act_pos']:.3f} vel={a0['act_vel']:.2f}"

def start_daemon(extra=()):
    p = subprocess.Popen([DAEMON, "--backend", "sim", "--axes", "2", "--cycle-ms", str(CYCLE_MS),
                          "--trace-seconds", "0", "--cmd-timeout-ms", "1000", "--shutdown-timeout-ms", "2000", *extra],
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    return p

print("== start daemon #1")
p = start_daemon()
time.sleep(0.5)
data = open_map("plc_data", DATA_SIZE, False)
cmd = open_map("plc_cmd", CMD_SIZE, True)
d = wait_for(lambda d: d["cycle"] > 10, 2, "daemon publishing")
check(d is not None and d["cycle"] > 10, "plc_data publishing (cycle advancing)")
check(d is not None and d["alarm_flags"] == 0, "no alarms at idle")
w = CmdWriter(cmd)

print("== enable axis 0")
w.axis(0, ENABLE)
d = wait_for(lambda d: d["axes"][0]["step"] == 20, 1, "READY")
check(d["axes"][0]["step"] == 20 and d["axes"][0]["flags"] & ST_ENABLED, f"axis0 READY + ENABLED ({summ(d)})")

print("== jog axis 0 (refresh every 250 ms for 0.6 s)")
for _ in range(3):
    w.axis(0, JOG_POS, jog_vel=10.0)
    time.sleep(0.2)
d = read_data(data)
check(d["axes"][0]["step"] == 30 and d["axes"][0]["act_vel"] > 5.0, f"axis0 jogging ({summ(d)})")
pos_at_release = d["axes"][0]["act_pos"]

print("== stop refreshing: dead-man must strip jog within ~1 s")
t0 = time.time()
d = wait_for(lambda d: d["alarm_flags"] & ALARM_CMD_TIMEOUT, 2.5, "cmd timeout alarm")
check(d["alarm_flags"] & ALARM_CMD_TIMEOUT, f"alarm bit1 set after {time.time()-t0:.2f}s ({summ(d)})")
d = wait_for(lambda d: d["axes"][0]["step"] == 20 and abs(d["axes"][0]["act_vel"]) < 0.01, 2, "axis stopped")
check(d["axes"][0]["step"] == 20, f"axis0 back to READY via dead-man ({summ(d)})")
w.axis(0, 0)
d = wait_for(lambda d: d["alarm_flags"] == 0, 1, "alarm cleared")
check(d["alarm_flags"] == 0, "alarm cleared by fresh message")

print("== MoveAbs axis 0 to +5")
w.axis(0, MOVE_ABS, pos=5.0, vel=50.0)
d = wait_for(lambda d: d["axes"][0]["step"] == 40, 1, "MOVE_ABS")
check(d["axes"][0]["step"] == 40, f"axis0 in MOVE_ABS ({summ(d)})")
d = wait_for(lambda d: d["axes"][0]["step"] == 20 and abs(d["axes"][0]["act_pos"] - 5.0) < 0.05, 5, "arrive")
check(abs(d["axes"][0]["act_pos"] - 5.0) < 0.05 and d["axes"][0]["step"] == 20, f"axis0 arrived at 5 ({summ(d)})")

print("== unrelated message (touch only axis 1): axis 0 must NOT re-dispatch")
saw_redispatch = False
for _ in range(5):
    w.axis(1, ENABLE)
    for _ in range(10):
        d = read_data(data)
        if d["axes"][0]["step"] != 20:
            saw_redispatch = True
        time.sleep(0.01)
check(not saw_redispatch, "axis0 stayed READY across 5 unrelated messages (per-axis fresh)")

print("== message touching axis 0 with the latched MOVE_ABS word: re-dispatch allowed")
w.touch_only(0)
d = wait_for(lambda d: d["axes"][0]["step"] in (40, 90, 91), 0.5, "re-dispatch")
check(d["axes"][0]["step"] in (40, 90, 91, 20), f"axis0 re-dispatched then settles ({summ(d)})")
wait_for(lambda d: d["axes"][0]["step"] == 20, 2, "settle")

print("== EMS while jogging: must stop and stay stopped")
for _ in range(2):
    w.axis(0, JOG_POS, jog_vel=10.0)
    time.sleep(0.2)
w.machine = 2  # EMS
w.publish(0)
d = wait_for(lambda d: abs(d["axes"][0]["act_vel"]) < 0.01, 2, "EMS standstill")
check(abs(d["axes"][0]["act_vel"]) < 0.01 and d["axes"][0]["step"] in (90, 91), f"EMS stopped axis0 ({summ(d)})")
w.machine = 1  # Reset releases EMS
w.axes[0]["word"] = 0
w.publish(1)
wait_for(lambda d: d["axes"][0]["step"] == 20, 2, "READY after EMS")

print("== SIGTERM: graceful shutdown")
for _ in range(2):
    w.axis(0, JOG_POS, jog_vel=10.0)
    time.sleep(0.2)
t0 = time.time()
p.send_signal(signal.SIGTERM)
try:
    rc = p.wait(timeout=8)
except subprocess.TimeoutExpired:
    p.kill(); rc = -9
out = p.stdout.read()
check(rc == 0, f"daemon exit code {rc} after {time.time()-t0:.2f}s")
check("phase 1" in out.lower() or "decel" in out.lower(), "shutdown phase 1 logged")
check("phase 2" in out.lower() or "disable" in out.lower(), "shutdown phase 2 logged")
last = read_data(data)
check(last is not None and abs(last["axes"][0]["act_vel"]) < 0.01, f"axis at standstill in last published data ({summ(last)})")
print("--- daemon #1 log tail ---")
print("\n".join(out.strip().splitlines()[-12:]))

print("== restart daemon #2 with our old mappings still open (inode reuse)")
ino_before = os.stat("/dev/shm/plc_data").st_ino
p = start_daemon()
time.sleep(0.5)
ino_after = os.stat("/dev/shm/plc_data").st_ino
check(ino_before == ino_after, f"plc_data inode unchanged across restart ({ino_before} == {ino_after})")
c0 = read_data(data)["cycle"] if read_data(data) else -1
time.sleep(0.3)
d = read_data(data)
check(d is not None and d["cycle"] > c0 and d["cycle"] < 1000, f"old mapping sees the NEW daemon's counter restarting low ({c0} -> {d['cycle'] if d else None})")
check(d is not None and d["axes"][0]["step"] == 0, "fresh daemon starts axis0 in IDLE (payload reset, no stale command latched)")
w2 = CmdWriter(cmd, cycle=w.cycle)
w2.axis(0, ENABLE)
d = wait_for(lambda d: d["axes"][0]["step"] == 20, 1, "READY on daemon #2 via old cmd mapping")
check(d["axes"][0]["step"] == 20, f"daemon #2 accepts commands through the bridge's old mapping ({summ(d)})")
p.send_signal(signal.SIGTERM)
rc = p.wait(timeout=8)
check(rc == 0, f"daemon #2 exit code {rc}")

print()
print("RESULT:", "PASS" if not failures else f"FAIL ({len(failures)})")
for f in failures:
    print("  -", f)
sys.exit(1 if failures else 0)
