//! The Single parameter table (Rack addendum OS 2.0, Single Dump Table pp. 13–21; SPEC §4.1–4.2).
//!
//! Data byte `n` of a single dump is parameter A`n` (`n < 128`); byte `128 + n` is B`n`.
//! Rows the addendum doesn't document are absent from [`SINGLE_PARAMS`] and must be preserved.

use crate::sysex::Page;
use crate::types::{ASSIGN_SOURCE_NAMES, CATEGORY_NAMES};

/// Parameter class from the addendum's "Class" column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamClass {
    /// `p`: performance controller (mod wheel, volume, pedals…). Shares a CC number but is not stored in the sound.
    Performance,
    /// `a` / `b`: sound parameter stored in the single dump.
    Sound,
    /// Internal value the editor must not change (A0 Sound Version).
    Internal,
}

/// Additional class flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ParamFlags {
    /// `ms`: stored with the Multi when in Multi mode.
    pub multi_single: bool,
    /// `np`: not part-sensitive.
    pub non_part_sensitive: bool,
}

impl ParamFlags {
    pub const NONE: Self = Self { multi_single: false, non_part_sensitive: false };
}

/// How a raw 0..127 byte is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueKind {
    /// Plain number.
    Unipolar,
    /// Stored 0..127, shown −64..+63 (64 = 0).
    Bipolar,
    /// 0 = Off, 1 = On.
    Toggle,
    /// Value texts indexed by raw value. Raw values past the end of the list are valid and preserved.
    Choice(&'static [&'static str]),
    /// MIDI note number, 0 = C-1.
    Note,
    /// ASCII name character.
    NameChar,
    /// Number whose meaning is not (yet) known well enough to label (e.g. unverified destination lists).
    Raw,
}

/// One row of the Single Dump table.
///
/// `min`/`max` are the **documented** range. Factory data exceeds it for some parameters
/// (A49 ≤ 30, A112 ≤ 29, destinations ≤ 114), so never clamp stored bytes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamDef {
    pub page: Page,
    pub number: u8,
    pub name: &'static str,
    pub section: &'static str,
    pub class: ParamClass,
    pub flags: ParamFlags,
    pub kind: ValueKind,
    pub min: u8,
    pub max: u8,
}

impl ParamDef {
    /// Offset of this parameter in the 256-byte single dump; `None` for Page C.
    pub fn dump_offset(&self) -> Option<usize> {
        self.page.dump_offset().map(|base| base + self.number as usize)
    }

    pub fn is_sound(&self) -> bool {
        self.class == ParamClass::Sound
    }

    pub fn format(&self, raw: u8) -> String {
        format_value(self, raw)
    }

    const fn ms(mut self) -> Self {
        self.flags.multi_single = true;
        self
    }

    const fn np(mut self) -> Self {
        self.flags.non_part_sensitive = true;
        self
    }

    const fn range(mut self, min: u8, max: u8) -> Self {
        self.min = min;
        self.max = max;
        self
    }
}

/// Formats a raw value for display: bipolar as `+n`/`-n`, choices by name (or `#raw` when
/// the value is beyond the documented list), notes as `C-1`…`G9`.
pub fn format_value(def: &ParamDef, raw: u8) -> String {
    format_kind(def.kind, raw)
}

/// [`format_value`] for a bare [`ValueKind`] (also used by the Multi table).
pub fn format_kind(kind: ValueKind, raw: u8) -> String {
    match kind {
        ValueKind::Unipolar | ValueKind::Raw => raw.to_string(),
        ValueKind::Bipolar => match raw as i16 - 64 {
            v if v > 0 => format!("+{v}"),
            v => v.to_string(),
        },
        ValueKind::Toggle => match raw {
            0 => "Off".to_owned(),
            1 => "On".to_owned(),
            other => format!("#{other}"),
        },
        ValueKind::Choice(texts) => match texts.get(raw as usize) {
            Some(text) => (*text).to_owned(),
            None => format!("#{raw}"),
        },
        ValueKind::Note => {
            const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
            format!("{}{}", NAMES[raw as usize % 12], raw as i32 / 12 - 1)
        }
        ValueKind::NameChar => match raw {
            32..=126 => (raw as char).to_string(),
            other => format!("#{other}"),
        },
    }
}

/// Looks up a documented Page A / Page B single parameter.
pub fn single_param(page: Page, number: u8) -> Option<&'static ParamDef> {
    SINGLE_PARAMS.iter().find(|def| def.page == page && def.number == number)
}

/// True only for documented sound parameters (class `a`/`b`); false for performance
/// controllers, A0 Sound Version, undocumented bytes and Page C.
pub fn is_sound_param(page: Page, number: u8) -> bool {
    single_param(page, number).is_some_and(ParamDef::is_sound)
}

// ---------------------------------------------------------------------------------------------
// Value lists
// ---------------------------------------------------------------------------------------------

pub const WAVE_SELECT_NAMES: [&str; 64] = [
    "Sine", "Triangle", "Wave 3", "Wave 4", "Wave 5", "Wave 6", "Wave 7", "Wave 8", "Wave 9",
    "Wave 10", "Wave 11", "Wave 12", "Wave 13", "Wave 14", "Wave 15", "Wave 16", "Wave 17",
    "Wave 18", "Wave 19", "Wave 20", "Wave 21", "Wave 22", "Wave 23", "Wave 24", "Wave 25",
    "Wave 26", "Wave 27", "Wave 28", "Wave 29", "Wave 30", "Wave 31", "Wave 32", "Wave 33",
    "Wave 34", "Wave 35", "Wave 36", "Wave 37", "Wave 38", "Wave 39", "Wave 40", "Wave 41",
    "Wave 42", "Wave 43", "Wave 44", "Wave 45", "Wave 46", "Wave 47", "Wave 48", "Wave 49",
    "Wave 50", "Wave 51", "Wave 52", "Wave 53", "Wave 54", "Wave 55", "Wave 56", "Wave 57",
    "Wave 58", "Wave 59", "Wave 60", "Wave 61", "Wave 62", "Wave 63", "Wave 64",
];
pub const SUB_OSC_SHAPE_NAMES: [&str; 2] = ["Square", "Triangle"];
pub const SATURATION_CURVE_NAMES: [&str; 7] = ["Off", "Light", "Soft", "Middle", "Hard", "Digital", "Shaper"];
pub const FILTER_MODE_NAMES: [&str; 4] = ["LP", "HP", "BP", "BS"];
pub const FILTER_ROUTING_NAMES: [&str; 4] = ["Ser4", "Ser6", "Par4", "Split"];
pub const LFO_SHAPE_NAMES: [&str; 6] = ["Sine", "Tri", "Saw", "Square", "S&H", "S&G"];
pub const LFO_MODE_NAMES: [&str; 2] = ["Poly", "Mono"];
pub const KEY_MODE_NAMES: [&str; 5] = ["Poly", "Mono 1", "Mono 2", "Mono 3", "Mono 4"];
/// Values 2..15 are documented only as numbers.
pub const UNISON_MODE_NAMES: [&str; 16] = [
    "Off", "Twin", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
];
pub const INPUT_MODE_NAMES: [&str; 4] = ["Off", "Dynamic", "Static", "ToEffects"];
/// Partial list; the documented range is 0..8 (Q3).
pub const INPUT_SELECT_NAMES: [&str; 3] = ["In1 L", "In1 L+R", "In1 R"];
/// Partial list; factory data uses values up to 29 (Q3).
pub const DELAY_REVERB_MODE_NAMES: [&str; 4] = ["Off", "Delay", "Reverb", "Rev+Feedb1"];
pub const ARP_MODE_NAMES: [&str; 7] = ["Off", "Up", "Down", "Up&Down", "AsPlayed", "Random", "Chord"];
pub const LFO3_DESTINATION_NAMES: [&str; 7] = ["Osc1", "Osc1+2", "Osc2", "PW1", "PW1+2", "PW2", "SyncPhase"];
pub const SMOOTH_MODE_NAMES: [&str; 4] = ["Off", "On", "Auto", "Note"];
pub const BENDER_SCALE_NAMES: [&str; 2] = ["Linear", "Exponential"];
pub const ENV_POLARITY_NAMES: [&str; 2] = ["Negative", "Positive"];
/// Partial list; the documented range is 0..12 (Q3).
pub const VOCODER_MODE_NAMES: [&str; 6] = ["Off", "Osc", "OscHold", "Noise", "In L", "In L+R"];
pub const PHASER_MODE_NAMES: [&str; 7] = ["Off", "1", "2", "3", "4", "5", "6"];
pub const FILTER_SELECT_NAMES: [&str; 3] = ["Filt1", "Filt2", "Filt1+2"];

/// Assign / LFO-assign destination names **in the order printed** in the OSR15 addendum.
///
/// UNVERIFIED (open question Q1): the printed order is not known to be the raw value
/// mapping (factory patches use values up to 114). Use for display hints only; always
/// store and show the raw value.
pub const UNVERIFIED_ASSIGN_DESTINATION_NAMES: &[&str] = &[
    "Off", "PatchVol", "ChannelVol", "Panorama", "Transpose", "Portamento", "Osc1Shape",
    "Osc1PlsWdh", "Osc1WavSel", "Osc1Pitch", "Osc1Keyflw", "Osc2Shape", "Osc2PlsWdh",
    "Osc2WavSel", "Osc2Pitch", "Osc2Detune", "Osc2FmAmt", "Osc2EnvAmt", "FmEnvAmt", "Osc2Keyflw",
    "OscBalance", "SubOscVol", "OscMainVol", "NoiseVol", "Cutoff", "Cutoff2", "Filt1Reso",
    "Filt2Reso", "Flt1EnvAmt", "Flt2EnvAmt", "Flt1Keyflw", "Flt2Keyflw", "FltBalance",
    "FltAttack", "FltDecay", "FltSustain", "FltSusTime", "FltRelease", "AmpAttack", "AmpDecay",
    "AmpSustain", "AmpSusTime", "AmpRelease", "Lfo1Rate", "Lfo1Cont", "Lfo1>Osc1", "Lfo1>Osc2",
    "Lfo1>PlsWd", "Lfo1>Reso", "Lfo1>FltGn", "Lfo2Rate", "Lfo2Cont", "Lfo2>Shape", "Lfo2>Fm",
    "Lfo2>Cut1", "Lfo2>Cut2", "Lfo2>Pan", "Lfo3Rate", "Lfo3OscAmt", "UniDetune", "UniSpread",
    "UniLfoPhs", "ChorusMix", "ChorusRate", "ChorusDpth", "ChorusDly", "ChorusFeed", "EffectSend",
    "DelayTime", "DelayFeed", "DelayRate", "DelayDepth", "Osc1ShpVel", "Osc2ShpVel", "PlsWhdVel",
    "FmAmtVel", "Flt1EnvVel", "Flt2EnvVel", "Reso1Vel", "Reso2Vel", "AmpVel", "PanVel",
    "Ass1Amt1", "Ass2Amt1", "Ass2Amt2", "Ass3Amt1", "Ass3Amt2", "Ass3Amt3", "OscInitPhs",
    "PunchInt", "RingMod", "NoiseColor", "DelayColor", "ABoostInt", "ABoostTune", "Lfo1AssAmt",
    "Lfo2AssAmt", "RevbDecay", "RevDamping", "RevbColor", "RevPredely", "RevFeedbck",
    "ArpNoteLen", "ArpSwing", "ArpPattern",
];

/// Definable knob destination names in the order printed in the OSR15 addendum.
///
/// UNVERIFIED (Q1): OS 1.50 re-sorted this list in the UI; the raw mapping is unconfirmed.
pub const UNVERIFIED_DEFINABLE_DESTINATION_NAMES: &[&str] = &[
    "Off", "ModWheel", "Breath", "Contr3", "Foot", "Data", "Balance", "Contr9", "Expression",
    "Contr12", "Contr13", "Contr14", "Contr15", "Contr16", "PatchVolume", "ChannelVolume",
    "Panorama", "Transpose", "Portamento", "UnisonDetune", "UnisonPanSprd", "UnisonLfoPhase",
    "ChorusMix", "ChorusRate", "ChorusDepth", "ChorusDelay", "ChorusFeedback", "EffectSend",
    "DelayTime(ms)", "DelayFeedback", "DelayRate", "DelayDepth", "Osc1WavSelect",
    "Osc1PulseWidth", "Osc1Semitone", "Osc1Keyfollow", "Osc2WavSelect", "Osc2PulseWidth",
    "Osc2EnvAmount", "FmEnvAmount", "Osc2Keyfollow", "NoiseVolume", "Filt1Resonance",
    "Filt2Resonance", "Filt1EnvAmount", "Filt2EnvAmount", "Filt1Keyfollow", "Filt2Keyfollow",
    "Lfo1Symmetry", "Lfo1>Osc1", "Lfo1>Osc2", "Lfo1>PulsWidth", "Lfo1>Resonance",
    "Lfo1>FiltGain", "Lfo2Symmetry", "Lfo2>Shape", "Lfo2>FmAmount", "Lfo2>Cutoff1",
    "Lfo2>Cutoff2", "Lfo2>Panorama", "Lfo3Rate", "Lfo3OscAmount", "Osc1ShapeVel", "Osc2ShapeVel",
    "PulsWidthVel", "FmAmountVel", "Filt1EnvVel", "Filt2EnvVel", "Resonance1Vel",
    "Resonance2Vel", "AmplifierVel", "PanoramaVel", "Assign1Amt1", "Assign2Amt1", "Assign2Amt2",
    "Assign3Amt1", "Assign3Amt2", "Assign3Amt3", "ClockTempo", "InputThru", "OscInitPhase",
    "PunchIntensity", "Ringmodulator", "NoiseColor", "DelayColor", "AnalogBoostInt",
    "AnalogBstTune", "Lfo1AssignAmt", "Lfo2AssignAmt", "RevDecayTime", "ReverbDamping",
    "ReverbColor", "ReverbFeedback", "ArpMode", "ArpPattern", "ArpClock", "ArpNoteLength",
    "ArpSwing", "ArpOctaves", "ArpHold",
];

// ---------------------------------------------------------------------------------------------
// Assign layout (Page B)
// ---------------------------------------------------------------------------------------------

/// Page B layout of one Assign slot: the source parameter and its (destination, amount) pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssignLayout {
    pub source: u8,
    pub targets: &'static [(u8, u8)],
}

/// Assign 1 (1 destination), Assign 2 (2) and Assign 3 (3): B64–B78.
pub const ASSIGN_LAYOUT: [AssignLayout; 3] = [
    AssignLayout { source: 64, targets: &[(65, 66)] },
    AssignLayout { source: 67, targets: &[(68, 69), (70, 71)] },
    AssignLayout { source: 72, targets: &[(73, 74), (75, 76), (77, 78)] },
];

/// LFO1 / LFO2 assign (destination, amount): B79/B80 and B81/B82.
pub const LFO_ASSIGN_LAYOUT: [(u8, u8); 2] = [(79, 80), (81, 82)];

// ---------------------------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------------------------

const fn def(
    page: Page,
    number: u8,
    name: &'static str,
    section: &'static str,
    class: ParamClass,
    kind: ValueKind,
) -> ParamDef {
    ParamDef { page, number, name, section, class, flags: ParamFlags::NONE, kind, min: 0, max: 127 }
}

const fn uni(page: Page, number: u8, name: &'static str, section: &'static str) -> ParamDef {
    def(page, number, name, section, ParamClass::Sound, ValueKind::Unipolar)
}

const fn bip(page: Page, number: u8, name: &'static str, section: &'static str) -> ParamDef {
    def(page, number, name, section, ParamClass::Sound, ValueKind::Bipolar)
}

const fn tog(page: Page, number: u8, name: &'static str, section: &'static str) -> ParamDef {
    def(page, number, name, section, ParamClass::Sound, ValueKind::Toggle).range(0, 1)
}

const fn raw(page: Page, number: u8, name: &'static str, section: &'static str) -> ParamDef {
    def(page, number, name, section, ParamClass::Sound, ValueKind::Raw)
}

const fn choice(
    page: Page,
    number: u8,
    name: &'static str,
    section: &'static str,
    texts: &'static [&'static str],
) -> ParamDef {
    def(page, number, name, section, ParamClass::Sound, ValueKind::Choice(texts))
        .range(0, texts.len() as u8 - 1)
}

const fn perf(page: Page, number: u8, name: &'static str) -> ParamDef {
    def(page, number, name, "Performance", ParamClass::Performance, ValueKind::Unipolar)
}

const fn name_char(number: u8, name: &'static str) -> ParamDef {
    def(B, number, name, "Name", ParamClass::Sound, ValueKind::NameChar).range(32, 127)
}

const A: Page = Page::PageA;
const B: Page = Page::PageB;

/// Every documented Page A and Page B row of the OS 2.0 Single Dump table, sorted by page and number.
pub static SINGLE_PARAMS: &[ParamDef] = &[
    // ----- Page A -----
    def(A, 0, "Sound Version", "Internal", ParamClass::Internal, ValueKind::Raw),
    perf(A, 1, "Modulation Wheel"),
    perf(A, 2, "Breath Controller"),
    perf(A, 3, "Contr 3"),
    perf(A, 4, "Foot Controller"),
    uni(A, 5, "Portamento Time", "Common"),
    perf(A, 6, "Data Slider"),
    perf(A, 7, "Channel Volume"),
    perf(A, 8, "Balance"),
    perf(A, 9, "Contr 9"),
    bip(A, 10, "Panorama", "Common"),
    perf(A, 11, "Expression"),
    perf(A, 12, "Contr 12"),
    perf(A, 13, "Contr 13"),
    perf(A, 14, "Contr 14"),
    perf(A, 15, "Contr 15"),
    perf(A, 16, "Contr 16"),
    bip(A, 17, "Osc1 Shape", "Osc 1"),
    uni(A, 18, "Osc1 Pulsewidth", "Osc 1"),
    choice(A, 19, "Osc1 Wave Select", "Osc 1", &WAVE_SELECT_NAMES),
    bip(A, 20, "Osc1 Semitone", "Osc 1"),
    bip(A, 21, "Osc1 Keyfollow", "Osc 1"),
    bip(A, 22, "Osc2 Shape", "Osc 2"),
    uni(A, 23, "Osc2 Pulsewidth", "Osc 2"),
    choice(A, 24, "Osc2 Wave Select", "Osc 2", &WAVE_SELECT_NAMES),
    bip(A, 25, "Osc2 Semitone", "Osc 2"),
    uni(A, 26, "Osc2 Detune", "Osc 2"),
    uni(A, 27, "Osc2 FM Amount", "Osc 2"),
    tog(A, 28, "Osc2 Sync", "Osc 2"),
    bip(A, 29, "Osc2 Filt Env Amt", "Osc 2"),
    bip(A, 30, "FM Filt Env Amt", "Osc 2"),
    bip(A, 31, "Osc2 Keyfollow", "Osc 2"),
    perf(A, 32, "Bank Select").range(0, 3),
    bip(A, 33, "Osc Balance", "Oscillators"),
    uni(A, 34, "Suboscillator Volume", "Oscillators"),
    choice(A, 35, "Suboscillator Shape", "Oscillators", &SUB_OSC_SHAPE_NAMES),
    uni(A, 36, "Osc Mainvolume", "Oscillators"),
    uni(A, 37, "Noise Volume", "Oscillators"),
    uni(A, 38, "Ringmodulator Volume", "Oscillators"),
    bip(A, 39, "Noise Color", "Oscillators"),
    uni(A, 40, "Cutoff", "Filters"),
    bip(A, 41, "Cutoff2", "Filters"),
    uni(A, 42, "Filter1 Resonance", "Filters"),
    uni(A, 43, "Filter2 Resonance", "Filters"),
    uni(A, 44, "Filter1 Env Amt", "Filters"),
    uni(A, 45, "Filter2 Env Amt", "Filters"),
    bip(A, 46, "Filter1 Keyfollow", "Filters"),
    bip(A, 47, "Filter2 Keyfollow", "Filters"),
    bip(A, 48, "Filter Balance", "Filters"),
    choice(A, 49, "Saturation Curve", "Filters", &SATURATION_CURVE_NAMES),
    choice(A, 51, "Filter1 Mode", "Filters", &FILTER_MODE_NAMES),
    choice(A, 52, "Filter2 Mode", "Filters", &FILTER_MODE_NAMES),
    choice(A, 53, "Filter Routing", "Filters", &FILTER_ROUTING_NAMES),
    uni(A, 54, "Filter Env Attack", "Filter Env"),
    uni(A, 55, "Filter Env Decay", "Filter Env"),
    uni(A, 56, "Filter Env Sustain", "Filter Env"),
    bip(A, 57, "Filter Env Sustain Time", "Filter Env"),
    uni(A, 58, "Filter Env Release", "Filter Env"),
    uni(A, 59, "Amp Env Attack", "Amp Env"),
    uni(A, 60, "Amp Env Decay", "Amp Env"),
    uni(A, 61, "Amp Env Sustain", "Amp Env"),
    bip(A, 62, "Amp Env Sustain Time", "Amp Env"),
    uni(A, 63, "Amp Env Release", "Amp Env"),
    perf(A, 64, "Hold Pedal"),
    perf(A, 65, "Portamento Pedal"),
    perf(A, 66, "Sostenuto Pedal"),
    uni(A, 67, "Lfo1 Rate", "LFO 1"),
    choice(A, 68, "Lfo1 Shape", "LFO 1", &LFO_SHAPE_NAMES),
    tog(A, 69, "Lfo1 Env Mode", "LFO 1"),
    choice(A, 70, "Lfo1 Mode", "LFO 1", &LFO_MODE_NAMES),
    bip(A, 71, "Lfo1 Symmetry", "LFO 1"),
    uni(A, 72, "Lfo1 Keyfollow", "LFO 1"),
    uni(A, 73, "Lfo1 Keytrigger", "LFO 1"),
    bip(A, 74, "Osc1 Lfo1 Amount", "LFO 1"),
    bip(A, 75, "Osc2 Lfo1 Amount", "LFO 1"),
    bip(A, 76, "PW Lfo1 Amount", "LFO 1"),
    bip(A, 77, "Reso Lfo1 Amount", "LFO 1"),
    bip(A, 78, "FiltGain Lfo1 Amount", "LFO 1"),
    uni(A, 79, "Lfo2 Rate", "LFO 2"),
    choice(A, 80, "Lfo2 Shape", "LFO 2", &LFO_SHAPE_NAMES),
    tog(A, 81, "Lfo2 Env Mode", "LFO 2"),
    choice(A, 82, "Lfo2 Mode", "LFO 2", &LFO_MODE_NAMES),
    bip(A, 83, "Lfo2 Symmetry", "LFO 2"),
    uni(A, 84, "Lfo2 Keyfollow", "LFO 2"),
    uni(A, 85, "Lfo2 Keytrigger", "LFO 2"),
    bip(A, 86, "OscShape Lfo2 Amount", "LFO 2"),
    bip(A, 87, "FmAmount Lfo2 Amount", "LFO 2"),
    bip(A, 88, "Cutoff1 Lfo2 Amount", "LFO 2"),
    bip(A, 89, "Cutoff2 Lfo2 Amount", "LFO 2"),
    bip(A, 90, "Panorama Lfo2 Amount", "LFO 2"),
    uni(A, 91, "Patch Volume", "Common"),
    bip(A, 93, "Transpose", "Common"),
    choice(A, 94, "Key Mode", "Common", &KEY_MODE_NAMES),
    choice(A, 97, "Unison Mode", "Unison", &UNISON_MODE_NAMES),
    uni(A, 98, "Unison Detune", "Unison"),
    uni(A, 99, "Unison Panorama Spread", "Unison"),
    bip(A, 100, "Unison Lfo Phase", "Unison"),
    choice(A, 101, "Input Mode", "Input", &INPUT_MODE_NAMES),
    choice(A, 102, "Input Select", "Input", &INPUT_SELECT_NAMES).range(0, 8),
    uni(A, 105, "Chorus Mix", "Effects"),
    uni(A, 106, "Chorus Rate", "Effects"),
    uni(A, 107, "Chorus Depth", "Effects"),
    uni(A, 108, "Chorus Delay", "Effects"),
    bip(A, 109, "Chorus Feedback", "Effects"),
    choice(A, 110, "Chorus Lfo Shape", "Effects", &LFO_SHAPE_NAMES),
    choice(A, 112, "Delay/Reverb Mode", "Effects", &DELAY_REVERB_MODE_NAMES),
    uni(A, 113, "Effect Send", "Effects").ms(),
    uni(A, 114, "Delay Time", "Effects").ms().np(),
    uni(A, 115, "Delay Feedback", "Effects").ms().np(),
    uni(A, 116, "Delay Rate / Reverb Decay Time", "Effects").ms().np(),
    uni(A, 117, "Delay Depth / Reverb Room Size", "Effects").ms().np(),
    uni(A, 118, "Delay Lfo Shape / Reverb Damping", "Effects").ms().np(),
    bip(A, 119, "Delay Color", "Effects").ms().np(),
    perf(A, 123, "All Notes Off"),
    // ----- Page B -----
    choice(B, 1, "Arp Mode", "Arpeggiator", &ARP_MODE_NAMES),
    uni(B, 2, "Arp Pattern Select", "Arpeggiator").range(0, 31),
    uni(B, 3, "Arp Octave Range", "Arpeggiator").range(0, 3),
    tog(B, 4, "Arp Hold Enable", "Arpeggiator"),
    bip(B, 5, "Arp Note Length", "Arpeggiator"),
    uni(B, 6, "Arp Swing", "Arpeggiator"),
    uni(B, 7, "Lfo3 Rate", "LFO 3"),
    choice(B, 8, "Lfo3 Shape", "LFO 3", &LFO_SHAPE_NAMES),
    choice(B, 9, "Lfo3 Mode", "LFO 3", &LFO_MODE_NAMES),
    uni(B, 10, "Lfo3 Keyfollow", "LFO 3"),
    choice(B, 11, "Lfo3 Destination", "LFO 3", &LFO3_DESTINATION_NAMES),
    uni(B, 12, "Osc Lfo3 Amount", "LFO 3"),
    uni(B, 13, "Lfo3 Fade-In Time", "LFO 3"),
    uni(B, 16, "Clock Tempo", "Clock"),
    raw(B, 17, "Arp Clock", "Clock").range(1, 17),
    raw(B, 18, "Lfo1 Clock", "Clock").range(0, 19),
    raw(B, 19, "Lfo2 Clock", "Clock").range(0, 19),
    raw(B, 20, "Delay Clock", "Clock").range(0, 16).ms().np(),
    raw(B, 21, "Lfo3 Clock", "Clock").range(0, 19),
    choice(B, 25, "Control Smooth Mode", "Common", &SMOOTH_MODE_NAMES),
    bip(B, 26, "Bender Range Up", "Common"),
    bip(B, 27, "Bender Range Down", "Common"),
    choice(B, 28, "Bender Scale", "Common", &BENDER_SCALE_NAMES),
    choice(B, 30, "Filter1 Env Polarity", "Filters", &ENV_POLARITY_NAMES),
    choice(B, 31, "Filter2 Env Polarity", "Filters", &ENV_POLARITY_NAMES),
    tog(B, 32, "Filter2 Cutoff Link", "Filters"),
    def(B, 33, "Filter Keytrack Base", "Filters", ParamClass::Sound, ValueKind::Note),
    uni(B, 35, "Osc Init Phase", "Oscillators"),
    uni(B, 36, "Punch Intensity", "Amp Env"),
    choice(B, 39, "Vocoder Mode", "Vocoder", &VOCODER_MODE_NAMES).range(0, 12),
    bip(B, 47, "Osc1 Shape Velocity", "Velocity"),
    bip(B, 48, "Osc2 Shape Velocity", "Velocity"),
    bip(B, 49, "PulseWidth Velocity", "Velocity"),
    bip(B, 50, "Fm Amount Velocity", "Velocity"),
    bip(B, 54, "Filter1 EnvAmt Velocity", "Velocity"),
    bip(B, 55, "Filter2 EnvAmt Velocity", "Velocity"),
    bip(B, 56, "Resonance1 Velocity", "Velocity"),
    bip(B, 57, "Resonance2 Velocity", "Velocity"),
    bip(B, 60, "Amp Velocity", "Velocity"),
    bip(B, 61, "Panorama Velocity", "Velocity"),
    raw(B, 62, "Definable1 Single", "Definable"),
    raw(B, 63, "Definable2 Single", "Definable"),
    choice(B, 64, "Assign1 Source", "Assign", &ASSIGN_SOURCE_NAMES),
    raw(B, 65, "Assign1 Destination", "Assign"),
    bip(B, 66, "Assign1 Amount", "Assign"),
    choice(B, 67, "Assign2 Source", "Assign", &ASSIGN_SOURCE_NAMES),
    raw(B, 68, "Assign2 Destination1", "Assign"),
    bip(B, 69, "Assign2 Amount1", "Assign"),
    raw(B, 70, "Assign2 Destination2", "Assign"),
    bip(B, 71, "Assign2 Amount2", "Assign"),
    choice(B, 72, "Assign3 Source", "Assign", &ASSIGN_SOURCE_NAMES),
    raw(B, 73, "Assign3 Destination1", "Assign"),
    bip(B, 74, "Assign3 Amount1", "Assign"),
    raw(B, 75, "Assign3 Destination2", "Assign"),
    bip(B, 76, "Assign3 Amount2", "Assign"),
    raw(B, 77, "Assign3 Destination3", "Assign"),
    bip(B, 78, "Assign3 Amount3", "Assign"),
    raw(B, 79, "LFO1 Assign Dest", "Assign"),
    bip(B, 80, "LFO1 Assign Amount", "Assign"),
    raw(B, 81, "LFO2 Assign Dest", "Assign"),
    bip(B, 82, "LFO2 Assign Amount", "Assign"),
    choice(B, 84, "Phaser Mode", "Effects", &PHASER_MODE_NAMES),
    uni(B, 85, "Phaser Mix", "Effects"),
    uni(B, 86, "Phaser Rate", "Effects"),
    uni(B, 87, "Phaser Depth", "Effects"),
    bip(B, 88, "Phaser Feedback", "Effects"),
    uni(B, 89, "Phaser Spread", "Effects"),
    uni(B, 97, "Analog Boost Intensity", "Analog Boost"),
    uni(B, 98, "Analog Boost Tune", "Analog Boost"),
    uni(B, 99, "Input Ringmodulator", "Effects"),
    name_char(112, "Single Name Char1"),
    name_char(113, "Single Name Char2"),
    name_char(114, "Single Name Char3"),
    name_char(115, "Single Name Char4"),
    name_char(116, "Single Name Char5"),
    name_char(117, "Single Name Char6"),
    name_char(118, "Single Name Char7"),
    name_char(119, "Single Name Char8"),
    name_char(120, "Single Name Char9"),
    name_char(121, "Single Name Char10"),
    choice(B, 122, "Filter Select", "Filters", &FILTER_SELECT_NAMES),
    choice(B, 123, "Category1", "Category", &CATEGORY_NAMES),
    choice(B, 124, "Category2", "Category", &CATEGORY_NAMES),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_unique() {
        for pair in SINGLE_PARAMS.windows(2) {
            assert!(
                (pair[0].page, pair[0].number) < (pair[1].page, pair[1].number),
                "{} / {} out of order",
                pair[0].name,
                pair[1].name
            );
        }
    }

    #[test]
    fn performance_and_internal_are_not_sound_params() {
        assert!(!is_sound_param(Page::PageA, 0));
        assert!(!is_sound_param(Page::PageA, 1));
        assert!(!is_sound_param(Page::PageA, 7));
        assert!(!is_sound_param(Page::PageA, 92));
        assert!(!is_sound_param(Page::PageC, 40));
        assert!(is_sound_param(Page::PageA, 40));
        assert!(is_sound_param(Page::PageB, 84));
    }

    #[test]
    fn format_value_handles_out_of_range_values() {
        let pan = single_param(Page::PageA, 10).unwrap();
        assert_eq!(format_value(pan, 0), "-64");
        assert_eq!(format_value(pan, 64), "0");
        assert_eq!(format_value(pan, 127), "+63");

        let saturation = single_param(Page::PageA, 49).unwrap();
        assert_eq!(format_value(saturation, 2), "Soft");
        assert_eq!(format_value(saturation, 30), "#30");

        let base = single_param(Page::PageB, 33).unwrap();
        assert_eq!(format_value(base, 0), "C-1");
        assert_eq!(format_value(base, 60), "C4");
        assert_eq!(format_value(base, 127), "G9");
    }
}
