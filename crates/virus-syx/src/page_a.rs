//! Access Virus Page A parameter mapping (Sound Engine, Oscillators, Filters, Envelopes, LFOs, Name).

use crate::types::*;

pub const PAGE_A_LEN: usize = 128;
pub const NAME_OFFSET: usize = 118;
pub const NAME_LEN: usize = 10;

/// Page A parameter structure representing the core subtractive synthesis sound engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageA {
    // Oscillators
    pub osc1_shape: OscShape,
    pub osc1_wave_pw: Unipolar7,
    pub osc1_semi: Bipolar7,
    pub osc1_detune: Unipolar7,
    pub osc1_keyfollow: Bipolar7,

    pub osc2_shape: OscShape,
    pub osc2_wave_pw: Unipolar7,
    pub osc2_semi: Bipolar7,
    pub osc2_detune: Unipolar7,
    pub osc2_keyfollow: Bipolar7,
    pub osc2_fm_amount: Unipolar7,
    pub osc2_fm_mode: u8,
    pub osc_sync: bool,
    pub pitch_env_amount: Bipolar7,

    // Sub-Oscillator & Mixer
    pub sub_osc_shape: u8,
    pub sub_osc_volume: Unipolar7,
    pub noise_volume: Unipolar7,
    pub noise_color: Bipolar7,
    pub ring_mod_volume: Unipolar7,

    // Filters
    pub filter1_mode: FilterMode,
    pub filter1_cutoff: Unipolar7,
    pub filter1_resonance: Unipolar7,
    pub filter1_env_amount: Bipolar7,
    pub filter1_keyfollow: Bipolar7,

    pub filter2_mode: FilterMode,
    pub filter2_cutoff: Unipolar7,
    pub filter2_resonance: Unipolar7,
    pub filter2_env_amount: Bipolar7,
    pub filter2_keyfollow: Bipolar7,
    pub filter2_cutoff_link: bool,

    pub filter_balance: Bipolar7,
    pub filter_routing: FilterRouting,

    // Envelopes
    pub filter_attack: Unipolar7,
    pub filter_decay: Unipolar7,
    pub filter_sustain: Unipolar7,
    pub filter_time: Bipolar7,
    pub filter_release: Unipolar7,

    pub amp_attack: Unipolar7,
    pub amp_decay: Unipolar7,
    pub amp_sustain: Unipolar7,
    pub amp_time: Bipolar7,
    pub amp_release: Unipolar7,

    // LFOs
    pub lfo1_shape: LfoShape,
    pub lfo1_rate: Unipolar7,
    pub lfo1_contour: Bipolar7,
    pub lfo1_keyfollow: bool,
    pub lfo1_trigger: LfoTriggerMode,

    pub lfo2_shape: LfoShape,
    pub lfo2_rate: Unipolar7,
    pub lfo2_contour: Bipolar7,
    pub lfo2_keyfollow: bool,
    pub lfo2_trigger: LfoTriggerMode,

    // Effects & Sends
    pub distortion_type: DistortionType,
    pub distortion_intensity: Unipolar7,
    pub chorus_send: Unipolar7,
    pub delay_send: Unipolar7,

    // 10-character ASCII Patch Name
    pub name_raw: [u8; NAME_LEN],

    // Complete backing array preserving all unmodified bits
    pub raw: [u8; PAGE_A_LEN],
}

impl Default for PageA {
    fn default() -> Self {
        let mut raw = [0u8; PAGE_A_LEN];
        raw[NAME_OFFSET..NAME_OFFSET + NAME_LEN].copy_from_slice(b"Init Patch");
        let mut p = Self::from_bytes(&raw);
        p.osc1_shape = OscShape::Saw;
        p.osc1_semi = Bipolar7(0);
        p.filter1_cutoff = Unipolar7(127);
        p.amp_decay = Unipolar7(64);
        p.amp_sustain = Unipolar7(127);
        p.amp_release = Unipolar7(40);
        p.sync_raw_from_fields();
        p
    }
}

impl PageA {
    /// Constructs PageA from raw 128 bytes.
    pub fn from_bytes(bytes: &[u8; PAGE_A_LEN]) -> Self {
        let mut name_raw = [b' '; NAME_LEN];
        name_raw.copy_from_slice(&bytes[NAME_OFFSET..NAME_OFFSET + NAME_LEN]);

        Self {
            osc1_shape: OscShape::from_raw(bytes[0]),
            osc1_wave_pw: Unipolar7::new(bytes[1]),
            osc1_semi: Bipolar7::from_raw(bytes[2]),
            osc1_detune: Unipolar7::new(bytes[3]),
            osc1_keyfollow: Bipolar7::from_raw(bytes[4]),

            osc2_shape: OscShape::from_raw(bytes[8]),
            osc2_wave_pw: Unipolar7::new(bytes[9]),
            osc2_semi: Bipolar7::from_raw(bytes[10]),
            osc2_detune: Unipolar7::new(bytes[11]),
            osc2_keyfollow: Bipolar7::from_raw(bytes[12]),
            osc2_fm_amount: Unipolar7::new(bytes[13]),
            osc2_fm_mode: bytes[14] & 0x7F,
            osc_sync: (bytes[15] & 1) != 0,
            pitch_env_amount: Bipolar7::from_raw(bytes[16]),

            sub_osc_shape: bytes[20] & 0x7F,
            sub_osc_volume: Unipolar7::new(bytes[21]),
            noise_volume: Unipolar7::new(bytes[24]),
            noise_color: Bipolar7::from_raw(bytes[25]),
            ring_mod_volume: Unipolar7::new(bytes[26]),

            filter1_mode: FilterMode::from_raw(bytes[32]),
            filter1_cutoff: Unipolar7::new(bytes[33]),
            filter1_resonance: Unipolar7::new(bytes[34]),
            filter1_env_amount: Bipolar7::from_raw(bytes[35]),
            filter1_keyfollow: Bipolar7::from_raw(bytes[36]),

            filter2_mode: FilterMode::from_raw(bytes[40]),
            filter2_cutoff: Unipolar7::new(bytes[41]),
            filter2_resonance: Unipolar7::new(bytes[42]),
            filter2_env_amount: Bipolar7::from_raw(bytes[43]),
            filter2_keyfollow: Bipolar7::from_raw(bytes[44]),
            filter2_cutoff_link: (bytes[45] & 1) != 0,

            filter_balance: Bipolar7::from_raw(bytes[46]),
            filter_routing: FilterRouting::from_raw(bytes[47]),

            filter_attack: Unipolar7::new(bytes[48]),
            filter_decay: Unipolar7::new(bytes[49]),
            filter_sustain: Unipolar7::new(bytes[50]),
            filter_time: Bipolar7::from_raw(bytes[51]),
            filter_release: Unipolar7::new(bytes[52]),

            amp_attack: Unipolar7::new(bytes[56]),
            amp_decay: Unipolar7::new(bytes[57]),
            amp_sustain: Unipolar7::new(bytes[58]),
            amp_time: Bipolar7::from_raw(bytes[59]),
            amp_release: Unipolar7::new(bytes[60]),

            lfo1_shape: LfoShape::from_raw(bytes[64]),
            lfo1_rate: Unipolar7::new(bytes[65]),
            lfo1_contour: Bipolar7::from_raw(bytes[66]),
            lfo1_keyfollow: (bytes[67] & 1) != 0,
            lfo1_trigger: LfoTriggerMode::from_raw(bytes[68]),

            lfo2_shape: LfoShape::from_raw(bytes[72]),
            lfo2_rate: Unipolar7::new(bytes[73]),
            lfo2_contour: Bipolar7::from_raw(bytes[74]),
            lfo2_keyfollow: (bytes[75] & 1) != 0,
            lfo2_trigger: LfoTriggerMode::from_raw(bytes[76]),

            chorus_send: Unipolar7::new(bytes[88]),
            distortion_type: DistortionType::from_raw(bytes[90]),
            distortion_intensity: Unipolar7::new(bytes[91]),
            delay_send: Unipolar7::new(bytes[94]),

            name_raw,
            raw: *bytes,
        }
    }

    /// Synchronizes typed fields back into the raw 128-byte array.
    pub fn sync_raw_from_fields(&mut self) {
        self.raw[0] = self.osc1_shape.to_raw();
        self.raw[1] = self.osc1_wave_pw.raw();
        self.raw[2] = self.osc1_semi.to_raw();
        self.raw[3] = self.osc1_detune.raw();
        self.raw[4] = self.osc1_keyfollow.to_raw();

        self.raw[8] = self.osc2_shape.to_raw();
        self.raw[9] = self.osc2_wave_pw.raw();
        self.raw[10] = self.osc2_semi.to_raw();
        self.raw[11] = self.osc2_detune.raw();
        self.raw[12] = self.osc2_keyfollow.to_raw();
        self.raw[13] = self.osc2_fm_amount.raw();
        self.raw[14] = self.osc2_fm_mode & 0x7F;
        self.raw[15] = if self.osc_sync { 1 } else { 0 };
        self.raw[16] = self.pitch_env_amount.to_raw();

        self.raw[20] = self.sub_osc_shape & 0x7F;
        self.raw[21] = self.sub_osc_volume.raw();
        self.raw[24] = self.noise_volume.raw();
        self.raw[25] = self.noise_color.to_raw();
        self.raw[26] = self.ring_mod_volume.raw();

        self.raw[32] = self.filter1_mode.to_raw();
        self.raw[33] = self.filter1_cutoff.raw();
        self.raw[34] = self.filter1_resonance.raw();
        self.raw[35] = self.filter1_env_amount.to_raw();
        self.raw[36] = self.filter1_keyfollow.to_raw();

        self.raw[40] = self.filter2_mode.to_raw();
        self.raw[41] = self.filter2_cutoff.raw();
        self.raw[42] = self.filter2_resonance.raw();
        self.raw[43] = self.filter2_env_amount.to_raw();
        self.raw[44] = self.filter2_keyfollow.to_raw();
        self.raw[45] = if self.filter2_cutoff_link { 1 } else { 0 };

        self.raw[46] = self.filter_balance.to_raw();
        self.raw[47] = self.filter_routing.to_raw();

        self.raw[48] = self.filter_attack.raw();
        self.raw[49] = self.filter_decay.raw();
        self.raw[50] = self.filter_sustain.raw();
        self.raw[51] = self.filter_time.to_raw();
        self.raw[52] = self.filter_release.raw();

        self.raw[56] = self.amp_attack.raw();
        self.raw[57] = self.amp_decay.raw();
        self.raw[58] = self.amp_sustain.raw();
        self.raw[59] = self.amp_time.to_raw();
        self.raw[60] = self.amp_release.raw();

        self.raw[64] = self.lfo1_shape.to_raw();
        self.raw[65] = self.lfo1_rate.raw();
        self.raw[66] = self.lfo1_contour.to_raw();
        self.raw[67] = if self.lfo1_keyfollow { 1 } else { 0 };
        self.raw[68] = self.lfo1_trigger.to_raw();

        self.raw[72] = self.lfo2_shape.to_raw();
        self.raw[73] = self.lfo2_rate.raw();
        self.raw[74] = self.lfo2_contour.to_raw();
        self.raw[75] = if self.lfo2_keyfollow { 1 } else { 0 };
        self.raw[76] = self.lfo2_trigger.to_raw();

        self.raw[88] = self.chorus_send.raw();
        self.raw[90] = self.distortion_type.to_raw();
        self.raw[91] = self.distortion_intensity.raw();
        self.raw[94] = self.delay_send.raw();

        self.raw[NAME_OFFSET..NAME_OFFSET + NAME_LEN].copy_from_slice(&self.name_raw);
    }

    /// Serializes Page A to its raw 128-byte array representation.
    pub fn to_bytes(&mut self) -> [u8; PAGE_A_LEN] {
        self.sync_raw_from_fields();
        self.raw
    }

    /// Returns the patch name as a clean UTF-8 string with trailing spaces trimmed.
    pub fn name(&self) -> String {
        let s = String::from_utf8_lossy(&self.name_raw);
        s.trim_end().to_string()
    }

    /// Sets the 10-character patch name, truncating or space-padding as required.
    pub fn set_name(&mut self, new_name: &str) {
        let mut buf = [b' '; NAME_LEN];
        let bytes = new_name.as_bytes();
        let copy_len = bytes.len().min(NAME_LEN);
        buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        self.name_raw = buf;
        self.raw[NAME_OFFSET..NAME_OFFSET + NAME_LEN].copy_from_slice(&buf);
    }
}
