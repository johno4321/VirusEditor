//! Value types and lossless value enums for Access Virus parameters.
//!
//! Every enum here keeps an `Unknown(u8)` variant so that any raw byte read from a
//! dump can be written back unchanged (SPEC §5.1).

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
/// Raw `0` → `-64`, raw `64` → `0`, raw `127` → `+63`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bipolar7(pub i8);

impl Bipolar7 {
    pub const MIN: Self = Self(-64);
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(63);

    #[inline]
    pub fn from_raw(raw: u8) -> Self {
        Self((raw.min(127) as i8) - 64)
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

/// Defines a lossless raw-value enum: documented values get named variants, anything
/// else is kept as `Unknown(raw)`.
macro_rules! lossless_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $names:ident, $count:literal {
            $($raw:literal => $variant:ident = $text:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
            /// A value outside the documented list, preserved as-is.
            Unknown(u8),
        }

        /// Display names indexed by raw value.
        pub const $names: [&str; $count] = [$($text),+];

        impl $name {
            /// All documented values, in raw-value order.
            pub const ALL: [Self; $count] = [$(Self::$variant),+];

            pub fn from_raw(raw: u8) -> Self {
                match raw {
                    $($raw => Self::$variant,)+
                    other => Self::Unknown(other),
                }
            }

            pub fn to_raw(self) -> u8 {
                match self {
                    $(Self::$variant => $raw,)+
                    Self::Unknown(raw) => raw,
                }
            }

            /// Display name; `"Unknown"` for undocumented values (`Display` shows `#raw`).
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Unknown(_) => "Unknown",
                }
            }

            pub fn is_known(self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::from_raw(0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                if self.is_known() {
                    f.write_str(self.name())
                } else {
                    write!(f, "#{}", self.to_raw())
                }
            }
        }
    };
}

lossless_enum! {
    /// Sound category stored in B123 / B124 (manual p. 84, 0-based).
    Category, CATEGORY_NAMES, 17 {
        0 => Off = "Off",
        1 => Lead = "Lead",
        2 => Bass = "Bass",
        3 => Pad = "Pad",
        4 => Decay = "Decay",
        5 => Pluck = "Pluck",
        6 => Acid = "Acid",
        7 => Classic = "Classic",
        8 => Arpeggiator = "Arpeggiator",
        9 => Effects = "Effects",
        10 => Drums = "Drums",
        11 => Percussion = "Percussion",
        12 => Input = "Input",
        13 => Vocoder = "Vocoder",
        14 => Favourite1 = "Favourite 1",
        15 => Favourite2 = "Favourite 2",
        16 => Favourite3 = "Favourite 3",
    }
}

lossless_enum! {
    /// Assign 1/2/3 modulation source (B64 / B67 / B72), OSR15 addendum list.
    AssignSource, ASSIGN_SOURCE_NAMES, 28 {
        0 => Off = "Off",
        1 => PitchBnd = "PitchBnd",
        2 => ChanPres = "ChanPres",
        3 => ModWheel = "ModWheel",
        4 => Breath = "Breath",
        5 => Contr3 = "Contr3",
        6 => Foot = "Foot",
        7 => Data = "Data",
        8 => Balance = "Balance",
        9 => Contr9 = "Contr9",
        10 => Express = "Express",
        11 => Contr12 = "Contr12",
        12 => Contr13 = "Contr13",
        13 => Contr14 = "Contr14",
        14 => Contr15 = "Contr15",
        15 => Contr16 = "Contr16",
        16 => HoldPed = "HoldPed",
        17 => PortaSw = "PortaSw",
        18 => SostPed = "SostPed",
        19 => AmpEnv = "AmpEnv",
        20 => FiltEnv = "FiltEnv",
        21 => Lfo1 = "Lfo1",
        22 => Lfo2 = "Lfo2",
        23 => Lfo3 = "Lfo3",
        24 => VeloOn = "VeloOn",
        25 => VeloOff = "VeloOff",
        26 => KeyFlw = "KeyFlw",
        27 => Random = "Random",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_values_round_trip() {
        for raw in 0..=127u8 {
            assert_eq!(Category::from_raw(raw).to_raw(), raw);
            assert_eq!(AssignSource::from_raw(raw).to_raw(), raw);
        }
        assert_eq!(Category::from_raw(127), Category::Unknown(127));
        assert_eq!(Category::from_raw(127).to_string(), "#127");
        assert_eq!(Category::from_raw(5), Category::Pluck);
    }
}
