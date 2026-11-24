//! 7-bit checksum calculation for Access Virus System Exclusive messages.

/// Calculates the Access Virus 7-bit checksum over the given slice of bytes.
///
/// The Virus checksum covers all bytes following the Command byte up to (but not including)
/// the checksum byte itself. It is computed as:
///
/// $$\text{checksum} = \left(\sum \text{bytes}\right) \ \& \ 0\text{x}7\text{F}$$
#[inline]
pub fn calculate_checksum(bytes: &[u8]) -> u8 {
    (bytes.iter().map(|&b| b as u32).sum::<u32>() & 0x7F) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_checksum() {
        let data = [0u8; 10];
        assert_eq!(calculate_checksum(&data), 0);
    }

    #[test]
    fn test_known_checksum_modulo() {
        let data = [0x40, 0x40]; // 64 + 64 = 128 -> 128 & 0x7F = 0
        assert_eq!(calculate_checksum(&data), 0x00);

        let data2 = [0x40, 0x41]; // 64 + 65 = 129 -> 129 & 0x7F = 1
        assert_eq!(calculate_checksum(&data2), 0x01);
    }

    #[test]
    fn test_synthetic_virus_dump_checksum() {
        let mut payload = vec![0x00, 0x01]; // Bank 0, Prog 1
        payload.extend_from_slice(&[0x40; 256]); // 256 bytes of center value 64
        assert_eq!(calculate_checksum(&payload), 1);
    }
}
