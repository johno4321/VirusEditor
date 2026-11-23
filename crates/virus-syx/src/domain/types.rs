//! Value types and domain enums for Access Virus synthesizer parameters.

use std::fmt;

/// Unsigned 7-bit MIDI parameter value (`0..=127`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Unipolar7(pub u8);

impl Unipolar7 {
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(127);
    pub const CENTER: Self = Self(64);

    #[inline]
    pub fn new(val: u8) -> Self {
        Self(val.min(127))
    }

    #[inline]
    pub fn raw(self) -> u8 {
        self.0
    }

    #[inline]
    pub fn normalized(self) -> f32 {
        self.0 as f32 / 127.0
    }
}

impl fmt::Display for Unipolar7 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Signed bipolar 7-bit parameter value stored with a center offset of 64 (`-64..=+63`).
///
/// In Virus SysEx:
/// * Raw value `0`   -> `-64`
/// * Raw value `64`  -> `0`
/// * Raw value `127` -> `+63`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bipolar7(pub i8);

impl Bipolar7 {
    pub const MIN: Self = Self(-64);
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(63);

    #[inline]
    pub fn from_raw(raw: u8) -> Self {
        let clamped = raw.min(127);
        Self((clamped as i8) - 64)
    }

    #[inline]
    pub fn to_raw(self) -> u8 {
        ((self.0 as i16) + 64).clamp(0, 127) as u8
    }

    #[inline]
    pub fn new(val: i8) -> Self {
        Self(val.clamp(-64, 63))
    }

    #[inline]
    pub fn normalized(self) -> f32 {
        if self.0 >= 0 {
            self.0 as f32 / 63.0
        } else {
            self.0 as f32 / 64.0
        }
    }
}

impl fmt::Display for Bipolar7 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 > 0 {
            write!(f, "+{}", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

/// Primary oscillator waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum OscShape {
    Off = 0,
    #[default]
    Saw = 1,
    Pulse = 2,
    Sine = 3,
    Triangle = 4,
    Wave = 5,
}

impl OscShape {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Off,
            1 => Self::Saw,
            2 => Self::Pulse,
            3 => Self::Sine,
            4 => Self::Triangle,
            _ => Self::Wave,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// Filter mode configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum FilterMode {
    #[default]
    Lowpass = 0,
    Highpass = 1,
    Bandpass = 2,
    Notch = 3,
}

impl FilterMode {
    pub fn from_raw(raw: u8) -> Self {
        match raw & 0x03 {
            0 => Self::Lowpass,
            1 => Self::Highpass,
            2 => Self::Bandpass,
            _ => Self::Notch,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// Dual filter routing architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum FilterRouting {
    #[default]
    Serial4 = 0,
    Serial6 = 1,
    Parallel = 2,
    Split = 3,
}

impl FilterRouting {
    pub fn from_raw(raw: u8) -> Self {
        match raw & 0x03 {
            0 => Self::Serial4,
            1 => Self::Serial6,
            2 => Self::Parallel,
            _ => Self::Split,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// Low Frequency Oscillator shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum LfoShape {
    #[default]
    Sine = 0,
    Triangle = 1,
    Saw = 2,
    Square = 3,
    SampleAndHold = 4,
    SampleAndGlide = 5,
}

impl LfoShape {
    pub fn from_raw(raw: u8) -> Self {
        match raw % 6 {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::Saw,
            3 => Self::Square,
            4 => Self::SampleAndHold,
            _ => Self::SampleAndGlide,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// LFO voice assignment trigger modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum LfoTriggerMode {
    #[default]
    Poly = 0,
    Mono = 1,
}

impl LfoTriggerMode {
    pub fn from_raw(raw: u8) -> Self {
        if (raw & 1) == 0 {
            Self::Poly
        } else {
            Self::Mono
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// Modulation Matrix Sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum ModSource {
    #[default]
    Off = 0,
    Lfo1 = 1,
    Lfo2 = 2,
    FilterEnv = 3,
    AmpEnv = 4,
    Velocity = 5,
    Aftertouch = 6,
    ModWheel = 7,
    Breath = 8,
    Foot = 9,
    PitchBend = 10,
    MidiVolume = 11,
    Pan = 12,
    Expression = 13,
    Assign1 = 14,
    Assign2 = 15,
}

impl ModSource {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Lfo1,
            2 => Self::Lfo2,
            3 => Self::FilterEnv,
            4 => Self::AmpEnv,
            5 => Self::Velocity,
            6 => Self::Aftertouch,
            7 => Self::ModWheel,
            8 => Self::Breath,
            9 => Self::Foot,
            10 => Self::PitchBend,
            11 => Self::MidiVolume,
            12 => Self::Pan,
            13 => Self::Expression,
            14 => Self::Assign1,
            15 => Self::Assign2,
            _ => Self::Off,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// Modulation Matrix Destinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum ModDestination {
    #[default]
    Off = 0,
    Pitch1 = 1,
    Pitch2 = 2,
    Pitch1And2 = 3,
    PulseWidth1 = 4,
    PulseWidth2 = 5,
    Osc2Detune = 6,
    FmAmount = 7,
    Cutoff1 = 8,
    Cutoff2 = 9,
    Cutoff1And2 = 10,
    Resonance1 = 11,
    Resonance2 = 12,
    FilterBalance = 13,
    Pan = 14,
    Lfo1Rate = 15,
    Lfo2Rate = 16,
}

impl ModDestination {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Pitch1,
            2 => Self::Pitch2,
            3 => Self::Pitch1And2,
            4 => Self::PulseWidth1,
            5 => Self::PulseWidth2,
            6 => Self::Osc2Detune,
            7 => Self::FmAmount,
            8 => Self::Cutoff1,
            9 => Self::Cutoff2,
            10 => Self::Cutoff1And2,
            11 => Self::Resonance1,
            12 => Self::Resonance2,
            13 => Self::FilterBalance,
            14 => Self::Pan,
            15 => Self::Lfo1Rate,
            16 => Self::Lfo2Rate,
            _ => Self::Off,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// Arpeggiator playback modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum ArpMode {
    #[default]
    Off = 0,
    Up = 1,
    Down = 2,
    UpDown = 3,
    AsPlayed = 4,
    Random = 5,
    Chord = 6,
}

impl ArpMode {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Up,
            2 => Self::Down,
            3 => Self::UpDown,
            4 => Self::AsPlayed,
            5 => Self::Random,
            6 => Self::Chord,
            _ => Self::Off,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}

/// Saturation and distortion curve modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum DistortionType {
    #[default]
    Off = 0,
    Light = 1,
    Soft = 2,
    Medium = 3,
    Hard = 4,
    Digital = 5,
    WaveShaper = 6,
    Rectifier = 7,
    RateReducer = 8,
}

impl DistortionType {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Light,
            2 => Self::Soft,
            3 => Self::Medium,
            4 => Self::Hard,
            5 => Self::Digital,
            6 => Self::WaveShaper,
            7 => Self::Rectifier,
            8 => Self::RateReducer,
            _ => Self::Off,
        }
    }

    pub fn to_raw(self) -> u8 {
        self as u8
    }
}
