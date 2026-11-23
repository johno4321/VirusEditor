//! Core Access Virus B / Rack domain models, SysEx protocols, and parameter mappings.

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
