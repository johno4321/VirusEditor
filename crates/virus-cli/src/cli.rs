//! Command-line interface definition (clap derive).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::address::{parse_address, parse_set_address, parse_value, ParamAddress};
use crate::probe::DestinationSlot;
use crate::settings::{parse_bank_letter, parse_channel, parse_device, parse_part, DeviceId, Part, Target};

/// Headless tool for live sessions with the Access Virus Rack (OS 2.00).
#[derive(Debug, Parser)]
#[command(name = "virus", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub conn: ConnArgs,
    #[command(subcommand)]
    pub command: Command,
}

/// Options shared by every command.
#[derive(Debug, Clone, Args)]
pub struct ConnArgs {
    /// MIDI input port: (part of) its name. Default: the only Virus-looking / real port.
    #[arg(long = "in", value_name = "PORT", global = true)]
    pub input: Option<String>,
    /// MIDI output port: (part of) its name. Default: the only Virus-looking / real port.
    #[arg(long = "out", value_name = "PORT", global = true)]
    pub output: Option<String>,
    /// SysEx device ID: 1-16 or omni (the Virus default since OS 1.50 is 1).
    #[arg(long, value_name = "1-16|omni", default_value = "1", value_parser = parse_device, global = true)]
    pub device: DeviceId,
    /// The synth's MIDI channel 1-16 (to interpret incoming CC / Poly Pressure).
    #[arg(long, value_name = "1-16", default_value = "1", value_parser = parse_channel, global = true)]
    pub channel: u8,
    /// Edit buffer: single (part 0x40, Single mode) or the Multi part 1-16.
    #[arg(long, value_name = "single|1-16", default_value = "single", value_parser = parse_part, global = true)]
    pub part: Part,
    /// How long to wait for an answer (bank requests: between two dumps), in ms.
    #[arg(long, value_name = "MS", default_value_t = 3000, global = true)]
    pub timeout_ms: u64,
    /// Pause after a parameter change or dump before reading back, in ms.
    #[arg(long, value_name = "MS", default_value_t = 60, global = true)]
    pub settle_ms: u64,
    /// Silence required on the MIDI input before each request, in ms. The Virus ignores
    /// requests that arrive too soon after it has sent or applied something.
    #[arg(long, value_name = "MS", default_value_t = 500, global = true)]
    pub quiet_ms: u64,
    /// Silence required before a bank request, in ms (a bank request right after another
    /// answer was ignored in the live test; 2 s worked).
    #[arg(long, value_name = "MS", default_value_t = 2000, global = true)]
    pub bank_quiet_ms: u64,
    /// Log every sent and received message as hex (to stderr).
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

impl ConnArgs {
    pub fn target(&self) -> Target {
        Target { device: self.device, channel: self.channel, part: self.part }
    }

    /// The port/device options as they'd be typed again, for printing suggested commands.
    pub fn repeat_options(&self) -> String {
        let mut options = String::new();
        if let Some(input) = &self.input {
            options.push_str(&format!(" --in \"{input}\""));
        }
        if let Some(output) = &self.output {
            options.push_str(&format!(" --out \"{output}\""));
        }
        if self.device != DeviceId::DEFAULT {
            let device = match self.device.0 {
                0x10 => "omni".to_owned(),
                n => (n + 1).to_string(),
            };
            options.push_str(&format!(" --device {device}"));
        }
        options
    }
}

#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// Only record the read-back values; don't ask what the display shows.
    #[arg(long)]
    pub non_interactive: bool,
    /// Don't request the edit buffer after each value (faster; no clamp detection).
    #[arg(long)]
    pub no_readback: bool,
    /// Continue an earlier run from --out-file (values already answered are skipped).
    #[arg(long)]
    pub resume: bool,
    /// Stop after this many values in a row that read back differently (0 = never).
    #[arg(long, value_name = "N", default_value_t = 3)]
    pub stop_after_rejected: u32,
    /// Where the original edit buffer is saved before probing.
    #[arg(long, value_name = "FILE", default_value = "probe_editbuffer_backup.syx")]
    pub backup_file: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List MIDI ports and mark the ones that look like a Virus.
    Ports,
    /// Print incoming MIDI, decoded (SysEx, parameter names, CC / Poly Pressure mapping).
    Monitor {
        /// Stop after this many seconds (default: until Ctrl+C).
        #[arg(long, value_name = "S")]
        seconds: Option<u64>,
    },
    /// Request the edit buffer (30 00 pp), print the patch and optionally save it.
    Fetch {
        /// Save the received dump exactly as received (.syx).
        #[arg(long, value_name = "FILE")]
        out_file: Option<PathBuf>,
        /// Print only the name, categories and sound version.
        #[arg(long)]
        brief: bool,
    },
    /// Send one patch from a .syx/.mid file to the edit buffer (10 00 pp).
    Send {
        file: PathBuf,
        /// Program number in the file (for bank files), 0-127.
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(0..=127))]
        program: Option<u8>,
        /// Which bank of the file, when it holds several (A-D).
        #[arg(long, value_name = "A-D", value_parser = parse_bank_letter)]
        from_bank: Option<u8>,
        /// Send even if the dump in the file has a wrong checksum (data may be damaged).
        #[arg(long)]
        allow_bad_checksum: bool,
        /// Fetch the edit buffer afterwards and compare byte by byte.
        #[arg(long)]
        verify: bool,
    },
    /// Request a whole bank (32 bb), check all 128 dumps and save them.
    GetBank {
        /// A-D (E-H: undocumented addressing, open question Q2).
        #[arg(value_parser = parse_bank_letter)]
        bank: u8,
        /// The .syx file to write (the dumps exactly as received, in program order).
        #[arg(long, value_name = "FILE")]
        out_file: PathBuf,
        /// Also save the bank into the editor's library as a new bank.
        #[arg(long)]
        to_library: bool,
        /// Library database (default: the editor's, %APPDATA%\VirusEditor\data\virus_library.db).
        #[arg(long, value_name = "FILE", requires = "to_library")]
        db: Option<PathBuf>,
        /// Write the file even when not all 128 programs arrived.
        #[arg(long)]
        allow_partial: bool,
    },
    /// Send a bank file to RAM bank A or B (overwrites it irrevocably).
    PutBank {
        /// A or B (C/D are ROM and refused).
        #[arg(value_parser = parse_bank_letter)]
        bank: u8,
        file: PathBuf,
        /// Confirm that the RAM bank on the synth will be overwritten.
        #[arg(long)]
        yes: bool,
        /// Pause between dumps in ms. The Virus sends its own bank dumps ≈340 ms apart (SPEC §3.7).
        #[arg(long, value_name = "MS", default_value_t = 300)]
        gap_ms: u64,
        /// Which bank of the file, when it holds several (A-D).
        #[arg(long, value_name = "A-D", value_parser = parse_bank_letter)]
        from_bank: Option<u8>,
        /// Send even when the file doesn't hold all 128 programs.
        #[arg(long)]
        allow_partial: bool,
        /// Send even if dumps in the file have wrong checksums (data may be damaged).
        #[arg(long)]
        allow_bad_checksum: bool,
        /// Skip the automatic backup of the target bank before sending.
        #[arg(long)]
        no_backup: bool,
        /// Request the bank back afterwards and compare every program.
        #[arg(long)]
        verify: bool,
    },
    /// Send one parameter change (70/71/72) to --part, e.g. `set A40 64`, `set A10 -5` or
    /// `set C32 1` (Page C: part, multi and global settings, SysEx only).
    Set {
        #[arg(value_parser = parse_set_address)]
        param: ParamAddress,
        /// 0-127, or a signed bipolar value like -5 / +10 (64 = 0).
        #[arg(value_parser = parse_value, allow_hyphen_values = true)]
        value: u8,
        /// Allow performance controllers, undocumented bytes and session-breaking globals
        /// (C93 Device ID, C122 Play Mode, C123 Part Number, C124 Global Channel, …). Never A0.
        #[arg(long)]
        force: bool,
        /// Fetch the edit buffer afterwards and check the value (Page A/B only).
        #[arg(long)]
        verify: bool,
    },
    /// Send one raw SysEx message for experiments, e.g. `send-hex "F0 00 20 33 01 00 72 40 20 01 F7"`.
    SendHex {
        /// The bytes in hex (two digits each; spaces optional). Must be F0 … F7 with 7-bit data.
        #[arg(required = true, num_args = 1..)]
        hex: Vec<String>,
        /// Really send it (without: only show what would be sent).
        #[arg(long)]
        yes: bool,
        /// Print whatever arrives for this long afterwards, in ms.
        #[arg(long, value_name = "MS", default_value_t = 1000)]
        listen_ms: u64,
    },
    /// Silence the synth: All Notes Off (CC123) and All Sound Off (CC120) on --channel, or on
    /// every channel with --all-channels. Needs only the output port.
    Panic {
        /// Send on all 16 channels instead of --channel.
        #[arg(long)]
        all_channels: bool,
    },
    /// Q1: step a destination through raw values and record what the display shows.
    ProbeDestinations {
        #[arg(long, value_enum, default_value = "assign1")]
        slot: DestinationSlot,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=127))]
        from: u8,
        #[arg(long, default_value_t = 127, value_parser = clap::value_parser!(u8).range(0..=127))]
        to: u8,
        #[arg(long, value_name = "FILE", default_value = "q1_destinations.json")]
        out_file: PathBuf,
        #[command(flatten)]
        probe: ProbeArgs,
    },
    /// Q3: step A49 / A112 / A102 / B39 (or another sound parameter) through raw values.
    ProbeValues {
        #[arg(value_parser = parse_address)]
        param: ParamAddress,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=127))]
        from: u8,
        /// Default: A49 40, A112 40, A102 15, B39 20, others 127.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=127))]
        to: Option<u8>,
        /// Default: q3_<param>.json.
        #[arg(long, value_name = "FILE")]
        out_file: Option<PathBuf>,
        #[command(flatten)]
        probe: ProbeArgs,
    },
    /// Turn a probe results file into a Rust name table (display readings by raw value).
    ProbeTable {
        file: PathBuf,
        #[arg(long, default_value = "VERIFIED_NAMES")]
        name: String,
    },
    /// Run the automatable G2 checks (fetch, parameter round trips, send, bank request).
    Selftest {
        /// Bank to request.
        #[arg(long, value_name = "A-D", default_value = "A", value_parser = parse_bank_letter)]
        bank: u8,
        #[arg(long)]
        skip_bank: bool,
        /// Don't send a Global request (35).
        #[arg(long)]
        skip_global: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_options_parse_anywhere_with_defaults() {
        let cli = Cli::try_parse_from(["virus", "set", "A10", "-5", "--device", "omni", "--part", "3"]).unwrap();
        assert_eq!(cli.conn.target(), Target { device: DeviceId(0x10), channel: 0, part: Part::Multi(2) });
        match cli.command {
            Command::Set { param, value, .. } => assert_eq!((param, value), (ParamAddress::a(10), 59)),
            other => panic!("unexpected {other:?}"),
        }
        let cli = Cli::try_parse_from(["virus", "--in", "UM", "fetch"]).unwrap();
        assert_eq!(cli.conn.target(), Target::default());
        assert_eq!(cli.conn.timeout_ms, 3000);
        assert!(Cli::try_parse_from(["virus", "get-bank", "A"]).is_err(), "--out-file is required");
        assert!(Cli::try_parse_from(["virus", "--device", "0", "ports"]).is_err());
    }

    #[test]
    fn panic_uses_the_global_channel_or_all_channels() {
        let cli = Cli::try_parse_from(["virus", "panic", "--channel", "11"]).unwrap();
        assert_eq!(cli.conn.channel, 10);
        assert!(matches!(cli.command, Command::Panic { all_channels: false }));
        let cli = Cli::try_parse_from(["virus", "panic", "--all-channels", "--out", "mioXC"]).unwrap();
        assert!(matches!(cli.command, Command::Panic { all_channels: true }));
        assert!(Cli::try_parse_from(["virus", "panic", "--channel", "17"]).is_err());
    }

    #[test]
    fn set_takes_page_c_and_send_hex_takes_split_or_quoted_bytes() {
        let cli = Cli::try_parse_from(["virus", "set", "C32", "1"]).unwrap();
        assert!(matches!(cli.command, Command::Set { param, value: 1, .. } if param == ParamAddress::c(32)));
        assert!(Cli::try_parse_from(["virus", "probe-values", "C32"]).is_err());

        let cli = Cli::try_parse_from(["virus", "send-hex", "F0 00", "20", "F7", "--yes"]).unwrap();
        match cli.command {
            Command::SendHex { hex, yes, listen_ms } => {
                assert_eq!((hex.join(" "), yes, listen_ms), ("F0 00 20 F7".to_owned(), true, 1000));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(Cli::try_parse_from(["virus", "send-hex"]).is_err());
    }

    #[test]
    fn repeat_options_reproduce_ports_and_device() {
        let cli = Cli::try_parse_from(["virus", "--in", "UM-ONE", "--out", "UM-ONE", "--device", "omni", "ports"]).unwrap();
        assert_eq!(cli.conn.repeat_options(), " --in \"UM-ONE\" --out \"UM-ONE\" --device omni");
        let cli = Cli::try_parse_from(["virus", "ports"]).unwrap();
        assert_eq!(cli.conn.repeat_options(), "");
    }
}
