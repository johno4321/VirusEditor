//! Access Virus Page B parameter mapping (Modulation Matrix, Arpeggiator, 3-Band EQ, Phaser, Extended FX).

use crate::types::*;

pub const PAGE_B_LEN: usize = 128;
pub const MATRIX_SLOTS_COUNT: usize = 6;

/// Single slot in the Access Virus Modulation Matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModMatrixSlot {
    pub source: ModSource,
    pub amount: Bipolar7,
    pub dest1: ModDestination,
    pub dest2: ModDestination,
}

/// Page B parameter structure representing extended synthesis features and multi-effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageB {
    // Arpeggiator
    pub arp_mode: ArpMode,
    pub arp_pattern: u8,
    pub arp_clock: u8,
    pub arp_note_length: u8,
    pub arp_swing: Bipolar7,

    // 6-Slot Modulation Matrix
    pub matrix: [ModMatrixSlot; MATRIX_SLOTS_COUNT],

    // Phaser
    pub phaser_stages: u8,
    pub phaser_rate: Unipolar7,
    pub phaser_depth: Unipolar7,
    pub phaser_feedback: Bipolar7,

    // 3-Band EQ
    pub eq_low_gain: Bipolar7,
    pub eq_low_freq: Unipolar7,
    pub eq_high_gain: Bipolar7,
    pub eq_high_freq: Unipolar7,
    pub eq_mid_gain: Bipolar7,
    pub eq_mid_freq: Unipolar7,
    pub eq_mid_q: Unipolar7,

    // Analog Boost
    pub boost_intensity: Unipolar7,
    pub boost_tune: Unipolar7,

    // Complete backing array preserving all unmodified bits
    pub raw: [u8; PAGE_B_LEN],
}

impl Default for PageB {
    fn default() -> Self {
        Self::from_bytes(&[0u8; PAGE_B_LEN])
    }
}

impl PageB {
    /// Constructs PageB from raw 128 bytes.
    pub fn from_bytes(bytes: &[u8; PAGE_B_LEN]) -> Self {
        let mut matrix = [ModMatrixSlot::default(); MATRIX_SLOTS_COUNT];
        for (i, slot) in matrix.iter_mut().enumerate() {
            let offset = 16 + (i * 4);
            if offset + 3 < PAGE_B_LEN {
                slot.source = ModSource::from_raw(bytes[offset]);
                slot.amount = Bipolar7::from_raw(bytes[offset + 1]);
                slot.dest1 = ModDestination::from_raw(bytes[offset + 2]);
                slot.dest2 = ModDestination::from_raw(bytes[offset + 3]);
            }
        }

        Self {
            arp_mode: ArpMode::from_raw(bytes[0]),
            arp_pattern: bytes[1] & 0x7F,
            arp_clock: bytes[2] & 0x7F,
            arp_note_length: bytes[3] & 0x7F,
            arp_swing: Bipolar7::from_raw(bytes[4]),

            matrix,

            phaser_stages: bytes[48] & 0x7F,
            phaser_rate: Unipolar7::new(bytes[49]),
            phaser_depth: Unipolar7::new(bytes[50]),
            phaser_feedback: Bipolar7::from_raw(bytes[51]),

            eq_low_gain: Bipolar7::from_raw(bytes[64]),
            eq_low_freq: Unipolar7::new(bytes[65]),
            eq_high_gain: Bipolar7::from_raw(bytes[66]),
            eq_high_freq: Unipolar7::new(bytes[67]),
            eq_mid_gain: Bipolar7::from_raw(bytes[68]),
            eq_mid_freq: Unipolar7::new(bytes[69]),
            eq_mid_q: Unipolar7::new(bytes[70]),

            boost_intensity: Unipolar7::new(bytes[72]),
            boost_tune: Unipolar7::new(bytes[73]),

            raw: *bytes,
        }
    }

    /// Synchronizes typed fields back into the raw 128-byte array.
    pub fn sync_raw_from_fields(&mut self) {
        self.raw[0] = self.arp_mode.to_raw();
        self.raw[1] = self.arp_pattern & 0x7F;
        self.raw[2] = self.arp_clock & 0x7F;
        self.raw[3] = self.arp_note_length & 0x7F;
        self.raw[4] = self.arp_swing.to_raw();

        for (i, slot) in self.matrix.iter().enumerate() {
            let offset = 16 + (i * 4);
            if offset + 3 < PAGE_B_LEN {
                self.raw[offset] = slot.source.to_raw();
                self.raw[offset + 1] = slot.amount.to_raw();
                self.raw[offset + 2] = slot.dest1.to_raw();
                self.raw[offset + 3] = slot.dest2.to_raw();
            }
        }

        self.raw[48] = self.phaser_stages & 0x7F;
        self.raw[49] = self.phaser_rate.raw();
        self.raw[50] = self.phaser_depth.raw();
        self.raw[51] = self.phaser_feedback.to_raw();

        self.raw[64] = self.eq_low_gain.to_raw();
        self.raw[65] = self.eq_low_freq.raw();
        self.raw[66] = self.eq_high_gain.to_raw();
        self.raw[67] = self.eq_high_freq.raw();
        self.raw[68] = self.eq_mid_gain.to_raw();
        self.raw[69] = self.eq_mid_freq.raw();
        self.raw[70] = self.eq_mid_q.raw();

        self.raw[72] = self.boost_intensity.raw();
        self.raw[73] = self.boost_tune.raw();
    }

    /// Serializes Page B to its raw 128-byte array representation.
    pub fn to_bytes(&mut self) -> [u8; PAGE_B_LEN] {
        self.sync_raw_from_fields();
        self.raw
    }
}
