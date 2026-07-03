//! CSP process-image layout: the PDO mapping we configure on every drive and
//! the (pure, allocation-free) encode/decode between raw PDI bytes and the
//! normalized image. EtherCAT is little-endian throughout.
//!
//! RxPDO (master → drive), 6 bytes:
//!   0x6040:00 u16 controlword | 0x607A:00 i32 target position
//! TxPDO (drive → master), 10 or 14 bytes:
//!   0x6041:00 u16 statusword | 0x6064:00 i32 position actual |
//!   0x606C:00 i32 velocity actual | [0x60FD:00 u32 digital inputs]
//!
//! 0x60FD digital inputs (CiA 402): bit0 = negative limit switch,
//! bit1 = positive limit switch, bit2 = home switch.

/// PDO mapping entries as 0x1600/0x1A00 sub-entries: index<<16 | sub<<8 | bits.
pub const RX_MAPPING: &[u32] = &[
    0x6040_0010, // controlword, 16 bit
    0x607A_0020, // target position, 32 bit
];

pub const TX_MAPPING_BASE: &[u32] = &[
    0x6041_0010, // statusword, 16 bit
    0x6064_0020, // position actual value, 32 bit
    0x606C_0020, // velocity actual value, 32 bit
];

/// Optional trailing TxPDO entry for limit/home switches.
pub const TX_MAPPING_DIN: u32 = 0x60FD_0020;

/// Standard CoE objects the adapter touches outside the PDI.
pub mod obj {
    /// RxPDO assign / TxPDO assign.
    pub const SM2_PDO_ASSIGN: u16 = 0x1C12;
    pub const SM3_PDO_ASSIGN: u16 = 0x1C13;
    /// First RxPDO / TxPDO mapping object.
    pub const RX_PDO_MAP: u16 = 0x1600;
    pub const TX_PDO_MAP: u16 = 0x1A00;
    /// Modes of operation (set to CSP = 8) and its display.
    pub const MODES_OF_OPERATION: u16 = 0x6060;
    pub const MODES_OF_OPERATION_DISPLAY: u16 = 0x6061;
    /// Error code (acyclic diagnostics).
    pub const ERROR_CODE: u16 = 0x603F;
    /// Cycle-synchronous position mode value.
    pub const MODE_CSP: u8 = 8;
}

pub const RX_BYTES: usize = 6;
pub const TX_BYTES_BASE: usize = 10;
pub const TX_BYTES_DIN: usize = 14;

pub fn tx_bytes(with_din: bool) -> usize {
    if with_din {
        TX_BYTES_DIN
    } else {
        TX_BYTES_BASE
    }
}

/// 0x60FD bit assignments.
pub const DIN_NEG_LIMIT: u32 = 1 << 0;
pub const DIN_POS_LIMIT: u32 = 1 << 1;
pub const DIN_HOME_SWITCH: u32 = 1 << 2;

/// Write one axis' outputs into its slice of the PDI. `buf` is exactly the
/// axis' RxPDO region ([`RX_BYTES`] long).
pub fn encode_rx(buf: &mut [u8], controlword: u16, target_position: i32) {
    buf[0..2].copy_from_slice(&controlword.to_le_bytes());
    buf[2..6].copy_from_slice(&target_position.to_le_bytes());
}

/// One axis' decoded inputs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxPdo {
    pub statusword: u16,
    pub position: i32,
    pub velocity: i32,
    /// Raw 0x60FD word; zero when digital inputs are not mapped.
    pub digital_inputs: u32,
}

/// Read one axis' inputs from its slice of the PDI. `buf` is exactly the
/// axis' TxPDO region (`tx_bytes(with_din)` long).
pub fn decode_tx(buf: &[u8], with_din: bool) -> TxPdo {
    TxPdo {
        statusword: u16::from_le_bytes([buf[0], buf[1]]),
        position: i32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]),
        velocity: i32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]),
        digital_inputs: if with_din {
            u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]])
        } else {
            0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rx_round_trip_bytes() {
        let mut buf = [0u8; RX_BYTES];
        encode_rx(&mut buf, 0x000F, -123_456);
        assert_eq!(buf[0..2], 0x000Fu16.to_le_bytes());
        assert_eq!(i32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]), -123_456);
    }

    #[test]
    fn tx_decode_with_and_without_din() {
        let mut buf = [0u8; TX_BYTES_DIN];
        buf[0..2].copy_from_slice(&0x0237u16.to_le_bytes());
        buf[2..6].copy_from_slice(&987_654i32.to_le_bytes());
        buf[6..10].copy_from_slice(&(-42_000i32).to_le_bytes());
        buf[10..14].copy_from_slice(&(DIN_POS_LIMIT | DIN_HOME_SWITCH).to_le_bytes());

        let full = decode_tx(&buf, true);
        assert_eq!(
            full,
            TxPdo {
                statusword: 0x0237,
                position: 987_654,
                velocity: -42_000,
                digital_inputs: DIN_POS_LIMIT | DIN_HOME_SWITCH,
            }
        );

        let base = decode_tx(&buf[..TX_BYTES_BASE], false);
        assert_eq!(base.digital_inputs, 0);
        assert_eq!(base.position, 987_654);
    }

    #[test]
    fn mapping_entries_encode_index_sub_bits() {
        assert_eq!(RX_MAPPING[0] >> 16, 0x6040);
        assert_eq!(RX_MAPPING[0] & 0xFF, 16);
        assert_eq!(TX_MAPPING_DIN >> 16, 0x60FD);
        assert_eq!(TX_MAPPING_DIN & 0xFF, 32);
        // region sizes match the mapping bit sums
        let rx_bits: u32 = RX_MAPPING.iter().map(|e| e & 0xFF).sum();
        assert_eq!(rx_bits as usize / 8, RX_BYTES);
        let tx_bits: u32 = TX_MAPPING_BASE.iter().map(|e| e & 0xFF).sum();
        assert_eq!(tx_bits as usize / 8, TX_BYTES_BASE);
        assert_eq!((tx_bits + (TX_MAPPING_DIN & 0xFF)) as usize / 8, TX_BYTES_DIN);
    }
}
