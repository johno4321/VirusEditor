//! Reading patches and banks from `.syx` / `.mid` files and writing `.syx` files.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use virus_syx::{extract_sysex_frames, parse_sysex, SysExMessage};

use crate::settings::bank_label;

/// A single dump found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDump {
    /// `bb`: 0 = edit buffer, 1..=4 = bank A..D.
    pub bank: u8,
    /// `ss`: program, or part for edit-buffer dumps.
    pub program: u8,
    pub data: [u8; 256],
    pub checksum_ok: bool,
}

/// The single dumps of a file plus a count of everything else in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedFile {
    pub singles: Vec<FileDump>,
    /// Multi dumps, other Virus messages, non-Virus and unparseable frames.
    pub other: usize,
}

impl LoadedFile {
    pub fn from_frames(frames: &[Vec<u8>]) -> Self {
        let mut file = Self::default();
        for frame in frames {
            match parse_sysex(frame) {
                Ok(SysExMessage::SingleDump {
                    bank,
                    program,
                    data,
                    checksum_ok,
                    ..
                }) => file.singles.push(FileDump {
                    bank,
                    program,
                    data: *data,
                    checksum_ok,
                }),
                _ => file.other += 1,
            }
        }
        file
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::from_frames(&extract_sysex_frames(bytes))
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        Ok(Self::from_bytes(&std::fs::read(path)?))
    }

    /// Distinct bank bytes (excluding the edit buffer) with their dump counts.
    pub fn banks(&self) -> BTreeMap<u8, usize> {
        let mut banks = BTreeMap::new();
        for dump in self.singles.iter().filter(|d| d.bank != 0) {
            *banks.entry(dump.bank).or_insert(0) += 1;
        }
        banks
    }

    /// Picks one single dump: the only one in the file, or the one for `program`
    /// (restricted to `from_bank` when the file holds several banks).
    pub fn pick_single(
        &self,
        program: Option<u8>,
        from_bank: Option<u8>,
    ) -> Result<&FileDump, String> {
        let in_bank = |d: &&FileDump| from_bank.is_none_or(|b| d.bank == b);
        match program {
            None => match self
                .singles
                .iter()
                .filter(in_bank)
                .collect::<Vec<_>>()
                .as_slice()
            {
                [only] => Ok(only),
                [] => Err("the file contains no single dump".to_owned()),
                many => Err(format!(
                    "the file contains {} single dumps; choose one with --program N",
                    many.len()
                )),
            },
            Some(program) => {
                let matches: Vec<&FileDump> = self
                    .singles
                    .iter()
                    .filter(|d| d.bank != 0 && d.program == program)
                    .filter(in_bank)
                    .collect();
                match matches.as_slice() {
                    [only] => Ok(only),
                    [] => Err(format!("no bank dump for program {program} in the file")),
                    many => Err(format!(
                        "program {program} exists in banks {}; choose one with --from-bank",
                        many.iter()
                            .map(|d| bank_label(d.bank))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }
            }
        }
    }

    /// The programs of one bank in program order. With several banks in the file,
    /// `from_bank` must choose one.
    pub fn pick_bank(&self, from_bank: Option<u8>) -> Result<BankSelection, String> {
        let banks = self.banks();
        let source = match from_bank {
            Some(bank) if banks.contains_key(&bank) => bank,
            Some(bank) => {
                return Err(format!(
                    "the file contains no dumps for bank {}",
                    bank_label(bank)
                ))
            }
            None => match banks.keys().copied().collect::<Vec<_>>().as_slice() {
                [only] => *only,
                [] => {
                    return Err(
                        "the file contains no bank dumps (only edit-buffer dumps or none)"
                            .to_owned(),
                    )
                }
                many => {
                    return Err(format!(
                        "the file contains banks {}; choose one with --from-bank",
                        many.iter()
                            .map(|&b| bank_label(b))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            },
        };
        let mut programs: BTreeMap<u8, &FileDump> = BTreeMap::new();
        let mut duplicates = Vec::new();
        for dump in self.singles.iter().filter(|d| d.bank == source) {
            if dump.program > 127 || programs.insert(dump.program, dump).is_some() {
                duplicates.push(dump.program);
            }
        }
        if !duplicates.is_empty() {
            return Err(format!(
                "bank {} has duplicate or invalid programs {duplicates:?}",
                bank_label(source)
            ));
        }
        Ok(BankSelection {
            source_bank: source,
            bad_checksums: programs
                .values()
                .filter(|d| !d.checksum_ok)
                .map(|d| d.program)
                .collect(),
            missing: (0..128u8).filter(|p| !programs.contains_key(p)).collect(),
            programs: programs.into_iter().map(|(p, d)| (p, d.data)).collect(),
        })
    }
}

/// One bank chosen from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankSelection {
    pub source_bank: u8,
    pub programs: Vec<(u8, [u8; 256])>,
    pub missing: Vec<u8>,
    pub bad_checksums: Vec<u8>,
}

/// Writes frames back to back as a `.syx` file.
pub fn write_syx(path: &Path, frames: &[&[u8]]) -> io::Result<()> {
    std::fs::write(path, frames.concat())
}

#[cfg(test)]
pub(crate) fn factory_bank(letter: char) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/factory")
        .join(format!("Bank_{letter}.syx"));
    std::fs::read(path).expect("factory bank file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::{encode_program_dump, Patch};

    #[test]
    fn factory_bank_selects_completely_with_valid_checksums() {
        let file = LoadedFile::from_bytes(&factory_bank('A'));
        assert_eq!(file.singles.len(), 128);
        let bank = file.pick_bank(None).unwrap();
        assert_eq!(bank.source_bank, 1);
        assert_eq!(bank.programs.len(), 128);
        assert!(bank.missing.is_empty() && bank.bad_checksums.is_empty());
        let smoog = file.pick_single(Some(1), None).unwrap();
        assert_eq!(Patch::from_bytes(&smoog.data).name(), "Smoog   OK");
        assert!(file.pick_single(None, None).is_err());
    }

    #[test]
    fn mixed_files_need_a_bank_choice() {
        let mut bytes = encode_program_dump(0x10, 1, 0, &[1; 256]);
        bytes.extend(encode_program_dump(0x10, 2, 0, &[2; 256]));
        let mut bad = encode_program_dump(0x10, 2, 1, &[3; 256]);
        bad[265] ^= 1;
        bytes.extend(bad);
        bytes.extend([0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x12, 0x01, 0xF7]);
        let file = LoadedFile::from_bytes(&bytes);
        assert_eq!(file.other, 1);
        assert!(file.pick_bank(None).unwrap_err().contains("A, B"));
        assert!(file
            .pick_single(Some(0), None)
            .unwrap_err()
            .contains("--from-bank"));
        assert_eq!(file.pick_single(Some(0), Some(2)).unwrap().data, [2; 256]);
        let b = file.pick_bank(Some(2)).unwrap();
        assert_eq!(b.programs.len(), 2);
        assert_eq!(b.missing.len(), 126);
        assert_eq!(b.bad_checksums, vec![1]);
        assert!(file.pick_bank(Some(3)).is_err());
    }

    #[test]
    fn single_patch_file_needs_no_program() {
        let file = LoadedFile::from_bytes(&encode_program_dump(0, 0, 0x40, &[5; 256]));
        assert_eq!(file.pick_single(None, None).unwrap().data, [5; 256]);
        assert!(file.pick_bank(None).is_err());
    }
}
