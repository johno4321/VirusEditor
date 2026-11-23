//! Core Access Virus B / Rack SysEx parser, parameter bitfield mappings, and patch models.
//!
//! Verified against official Access Virus Rack Manual (OSR1_Manual_International.pdf, Chapter 20).
//!
//! This crate provides zero-GC, deterministic parsing and serialization for:
//! * 7-bit Virus checksum validation and generation
//! * Complete 256-byte Single Program patches (Page A sound engine + Page B modulation/FX)
//! * Live parameter change SysEx encoding/decoding (Pages A [0x70], B [0x71], C [0x72])
//! * Dump request generation (Single, Bank, Multi, Arrangement, Total)
//! * Bipolar and Unipolar 7-bit synthesis value wrappers

#![forbid(unsafe_code)]

pub mod checksum;
pub mod types;
pub mod sysex;
pub mod page_a;
pub mod page_b;
pub mod patch;

pub use checksum::{calculate_checksum, verify_checksum};
pub use types::*;
pub use sysex::{
    encode_dump_request, encode_param_change, encode_program_dump, encode_single_bank_request,
    encode_total_request, parse_sysex, Command, Page, SyxError, SysExMessage,
    ACCESS_MANUFACTURER_ID, DEVICE_ID_OMNI, PART_SINGLE_BUFFER, PRODUCT_ID_VIRUS,
};
pub use page_a::{PageA, PAGE_A_LEN};
pub use page_b::{ModMatrixSlot, PageB, MATRIX_SLOTS_COUNT, PAGE_B_LEN};
pub use patch::{Patch, PATCH_TOTAL_LEN};
