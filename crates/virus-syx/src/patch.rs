//! Full 256-byte Access Virus Patch representation combining Page A and Page B.

use crate::page_a::{PageA, PAGE_A_LEN};
use crate::page_b::{PageB, PAGE_B_LEN};
use crate::sysex::{encode_program_dump, parse_sysex, SyxError, SysExMessage};

pub const PATCH_TOTAL_LEN: usize = PAGE_A_LEN + PAGE_B_LEN; // 256 bytes

/// Complete Access Virus Single Patch container.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Patch {
    pub page_a: PageA,
    pub page_b: PageB,
}

impl Patch {
    /// Creates a new Patch with standard default sound parameters and the given name.
    pub fn new(name: &str) -> Self {
        let mut patch = Self::default();
        patch.set_name(name);
        patch
    }

    /// Parses a complete 256-byte patch memory buffer.
    pub fn from_bytes(bytes: &[u8; PATCH_TOTAL_LEN]) -> Self {
        let mut a_bytes = [0u8; PAGE_A_LEN];
        let mut b_bytes = [0u8; PAGE_B_LEN];
        a_bytes.copy_from_slice(&bytes[..PAGE_A_LEN]);
        b_bytes.copy_from_slice(&bytes[PAGE_A_LEN..]);

        Self {
            page_a: PageA::from_bytes(&a_bytes),
            page_b: PageB::from_bytes(&b_bytes),
        }
    }

    /// Serializes the patch into a 256-byte continuous memory block.
    pub fn to_bytes(&mut self) -> [u8; PATCH_TOTAL_LEN] {
        let mut out = [0u8; PATCH_TOTAL_LEN];
        let a = self.page_a.to_bytes();
        let b = self.page_b.to_bytes();
        out[..PAGE_A_LEN].copy_from_slice(&a);
        out[PAGE_A_LEN..].copy_from_slice(&b);
        out
    }

    /// Parses a Patch directly from a complete raw Single Program Dump SysEx frame (`.syx`).
    /// Returns the parsed `Patch`, along with the embedded `bank` and `program` indices.
    pub fn from_sysex(msg: &[u8]) -> Result<(Self, u8, u8), SyxError> {
        let parsed = parse_sysex(msg)?;
        match parsed {
            SysExMessage::SingleProgramDump { bank, prog, data, .. } => {
                let patch = Self::from_bytes(&data);
                Ok((patch, bank, prog))
            }
            _ => Err(SyxError::InvalidPayloadLength {
                expected: 267,
                found: msg.len(),
            }),
        }
    }

    /// Encodes this Patch into a standard 267-byte Single Program Dump SysEx frame.
    pub fn to_sysex(&mut self, device_id: u8, bank: u8, prog: u8) -> Vec<u8> {
        let data = self.to_bytes();
        encode_program_dump(device_id, bank, prog, &data)
    }

    /// Convenient accessor for the patch name.
    pub fn name(&self) -> String {
        self.page_a.name()
    }

    /// Convenient setter for the patch name.
    pub fn set_name(&mut self, name: &str) {
        self.page_a.set_name(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn test_patch_name_and_roundtrip() {
        let mut patch = Patch::new("Moog Bass");
        assert_eq!(patch.name(), "Moog Bass");

        patch.page_a.filter1_cutoff = Unipolar7::new(88);
        patch.page_b.matrix[0].source = ModSource::Lfo1;
        patch.page_b.matrix[0].amount = Bipolar7::new(32);
        patch.page_b.matrix[0].dest1 = ModDestination::Cutoff1;

        let bytes = patch.to_bytes();
        assert_eq!(bytes.len(), 256);

        let mut roundtrip = Patch::from_bytes(&bytes);
        assert_eq!(roundtrip.name(), "Moog Bass");
        assert_eq!(roundtrip.page_a.filter1_cutoff, Unipolar7::new(88));
        assert_eq!(roundtrip.page_b.matrix[0].source, ModSource::Lfo1);
        assert_eq!(roundtrip.page_b.matrix[0].amount, Bipolar7::new(32));
        assert_eq!(roundtrip.page_b.matrix[0].dest1, ModDestination::Cutoff1);
    }

    #[test]
    fn test_patch_sysex_dump_encode_decode() {
        let mut patch = Patch::new("SuperLead");
        patch.page_a.osc1_shape = OscShape::Saw;
        patch.page_a.osc2_shape = OscShape::Pulse;
        patch.page_a.filter_routing = FilterRouting::Serial4;

        let syx = patch.to_sysex(0x10, 1, 42);
        assert_eq!(syx.len(), 267);

        let (decoded, bank, prog) = Patch::from_sysex(&syx).expect("SysEx should decode cleanly");
        assert_eq!(bank, 1);
        assert_eq!(prog, 42);
        assert_eq!(decoded.name(), "SuperLead");
        assert_eq!(decoded.page_a.osc1_shape, OscShape::Saw);
        assert_eq!(decoded.page_a.osc2_shape, OscShape::Pulse);
        assert_eq!(decoded.page_a.filter_routing, FilterRouting::Serial4);
    }
}
