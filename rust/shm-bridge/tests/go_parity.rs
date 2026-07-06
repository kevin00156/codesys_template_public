//! Cross-language layout proof: decode the golden fixtures produced by the
//! Go side (`backend/internal/shm/golden_test.go`, regenerated with
//! `go test ./backend/internal/shm -run Golden -update`) and check every
//! field, then re-encode and demand byte equality. If either direction
//! breaks, one side changed layout without the other (or without a Version
//! bump).

use shm_bridge::layout::{self, AxisCmd, AxisState, Header, PlcCommand, PlcData};
use shm_bridge::trace::{self, TraceAxisSample, TraceSample};

fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../backend/internal/shm/testdata");
    std::fs::read(format!("{path}/{name}")).unwrap_or_else(|e| {
        panic!(
            "missing fixture {name}: {e}\n\
             generate with: go test ./backend/internal/shm -run Golden -update"
        )
    })
}

/// Same formulas as Go's goldenData().
fn golden_data() -> PlcData {
    let mut d = PlcData::default();
    d.header = Header {
        magic: layout::PLC_DATA_MAGIC,
        version: layout::PLC_DATA_VERSION,
        flags: 0x5A5A,
        seq: 6,
        _pad: 0,
        cycle: 0x1122_3344_5566_7788,
    };
    d.system.temperature = 36.75;
    d.system.status_flags = 0xC0FF_EE01;
    d.system.alarm_flags = 0x0BAD_F00D;
    for (i, ax) in d.machine.axes.iter_mut().enumerate() {
        let i = i as f64;
        *ax = AxisState {
            act_pos: 1.5 + 100.0 * i,
            act_vel: -2.25 + 100.0 * i,
            set_pos: 3.125 + 100.0 * i,
            set_vel: -4.0625 + 100.0 * i,
            step: 10 + i as i32,
            flags: 0x21 + i as u32,
            error_id: -(100 + i as i32),
            _pad: 0,
        };
    }
    d.machine.run_state = 0x00C0_FFEE;
    d.machine.alarms = 0x0FAC_E0FF;
    d.production.n_production_state = -7;
    d
}

/// Same formulas as Go's goldenCmd().
fn golden_cmd() -> PlcCommand {
    let mut c = PlcCommand::default();
    c.header = Header {
        magic: layout::PLC_COMMAND_MAGIC,
        version: layout::PLC_COMMAND_VERSION,
        flags: 0xA5A5,
        seq: 8,
        _pad: 0,
        cycle: 0x8877_6655_4433_2211,
    };
    for (i, ax) in c.machine.axes.iter_mut().enumerate() {
        let i = i as f64;
        *ax = AxisCmd {
            control_flags: 0x41 + i as u32,
            _pad: 0,
            jog_vel: 5.5 + 10.0 * i,
            move_abs_pos: -6.25 + 10.0 * i,
            move_abs_vel: 7.75 + 10.0 * i,
        };
    }
    c.machine.control_flags = 5;
    c.production.n_production_state = 42;
    c
}

#[test]
fn plc_data_matches_go_bytes() {
    let bytes = fixture("plc_data_v4.bin");
    assert_eq!(bytes.len(), layout::SIZE_PLC_DATA);

    // decode: every field must equal the Go-side struct
    let decoded: PlcData = layout::from_bytes(&bytes);
    assert_eq!(decoded, golden_data());

    // encode: byte-identical round trip
    assert_eq!(layout::as_bytes(&golden_data()), &bytes[..]);
}

#[test]
fn plc_command_matches_go_bytes() {
    let bytes = fixture("plc_cmd_v3.bin");
    assert_eq!(bytes.len(), layout::SIZE_PLC_COMMAND);

    let decoded: PlcCommand = layout::from_bytes(&bytes);
    assert_eq!(decoded, golden_cmd());

    assert_eq!(layout::as_bytes(&golden_cmd()), &bytes[..]);
}

/// Same formulas as Go's goldenTraceSample().
fn golden_trace_sample() -> TraceSample {
    let mut s = TraceSample {
        cycle: 0x1122_3344_5566_7788,
        t_mono_ns: 0x0102_0304_0506_0708,
        period_ns: 2_000_123,
        exchange_ns: 123_456,
        bus_state: 3,
        status_bits: 0b101,
        _pad: 0,
        run_state: 0x00C0_FFEE,
        axes: Default::default(),
    };
    for (i, ax) in s.axes.iter_mut().enumerate() {
        let f = i as f64;
        *ax = TraceAxisSample {
            act_pos: 1.5 + 100.0 * f,
            act_vel: -2.25 + 100.0 * f,
            set_pos: 3.125 + 100.0 * f,
            set_vel: -4.0625 + 100.0 * f,
            step: 10 + i as i32,
            flags: 0x21 + i as u32,
            error_id: -(100 + i as i32),
            fault_code: 0x8000 + i as u32,
            drive_status: i as u32 + 1,
            io_bits: 0x11 + i as u32,
        };
    }
    s
}

#[test]
fn trace_sample_matches_go_bytes() {
    let bytes = fixture("plc_trace_v1.bin");
    assert_eq!(bytes.len(), trace::SIZE_TRACE_SAMPLE);

    let decoded = trace::sample_from_bytes(&bytes);
    assert_eq!(decoded, golden_trace_sample());

    assert_eq!(trace::sample_as_bytes(&golden_trace_sample()), &bytes[..]);
}
