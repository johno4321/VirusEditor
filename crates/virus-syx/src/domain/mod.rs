//! Access Virus Rack (OS 2.00) domain model: SysEx protocol, single parameter table and
//! the byte-exact 256-byte patch.

pub mod checksum;
pub mod file;
pub mod params;
pub mod patch;
pub mod sysex;
pub mod types;

pub use checksum::{calculate_checksum, verify_checksum};
pub use file::{extract_sysex_frames, read_sysex_file};
pub use params::{
    format_value, is_sound_param, single_param, ParamClass, ParamDef, ParamFlags, ValueKind,
    SINGLE_PARAMS, UNVERIFIED_ASSIGN_DESTINATION_NAMES, UNVERIFIED_DEFINABLE_DESTINATION_NAMES,
};
pub use patch::{AssignSlot, AssignTarget, DumpTarget, Patch, SingleBank, PATCH_TOTAL_LEN};
pub use sysex::{
    encode_arrangement_request, encode_controller_dump_request, encode_dump_request,
    encode_global_request, encode_multi_bank_request, encode_multi_dump,
    encode_multi_program_change, encode_multi_request, encode_param_change,
    encode_part_bank_change, encode_part_bank_select, encode_part_program_change,
    encode_program_dump, encode_single_bank_request, encode_total_request, parse_sysex, Command,
    Page, SysExMessage, SyxError, ACCESS_MANUFACTURER_ID, DEVICE_ID_OMNI, PART_SINGLE_BUFFER,
    PRODUCT_ID_VIRUS,
};
pub use types::*;
