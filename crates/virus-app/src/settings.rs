//! Settings kept between runs (G3): MIDI ports, channel, device ID, edited part, note audition,
//! last bank/tab, import/export folders, window geometry and "Select on synth".
//!
//! Stored as a small versioned JSON file (`settings.json`) next to the library database rather
//! than through eframe's `persistence` feature: that feature only offers an opaque key/value
//! store and window size, whereas this file lets us validate every field, move a corrupt file
//! aside with a visible notice, and drop saved ports that no longer exist. Everything except
//! the file I/O in [`SettingsStore`] is pure and unit-tested.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use virus_midi::{find_port_or_renamed, MidiCommand};
use virus_syx::{DEVICE_ID_OMNI, PART_SINGLE_BUFFER};

use crate::audition::{AuditionSettings, MAX_OCTAVE, MIN_OCTAVE};
use crate::session::{EditPart, MidiSettings, Ports};
use crate::storage::data_dir;

pub const SETTINGS_FILE_NAME: &str = "settings.json";
pub const CURRENT_VERSION: u32 = 1;
/// Changes are written at most this often (window drags change the settings every frame).
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub midi: MidiPrefs,
    pub audition: AuditionPrefs,
    pub ui: UiPrefs,
    pub folders: FolderPrefs,
    pub window: Option<WindowPrefs>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            midi: MidiPrefs::default(),
            audition: AuditionPrefs::default(),
            ui: UiPrefs::default(),
            folders: FolderPrefs::default(),
            window: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiPrefs {
    pub input: Option<String>,
    pub output: Option<String>,
    /// `false` once the user picked ports by hand.
    pub auto_connect: bool,
    /// 0..=15.
    pub channel: u8,
    /// 0..=15, or 0x10 = Omni.
    pub device_id: u8,
    /// SysEx part byte: 0x40 = Single, 0..=15 = Multi part.
    pub part: u8,
}

impl Default for MidiPrefs {
    fn default() -> Self {
        let defaults = MidiSettings::default();
        Self {
            input: None,
            output: None,
            auto_connect: true,
            channel: defaults.channel,
            device_id: defaults.device_id,
            part: defaults.part.raw(),
        }
    }
}

/// Hold (latch) is deliberately not stored: it always starts off, so a restart never begins
/// with a sustained note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditionPrefs {
    /// Keyboard strip expanded.
    pub open: bool,
    pub octave: i8,
    pub velocity_from_position: bool,
    pub velocity: u8,
    pub notes: Vec<u8>,
    pub length_ms: u64,
}

impl Default for AuditionPrefs {
    fn default() -> Self {
        let s = AuditionSettings::default();
        Self {
            open: true,
            octave: 0,
            velocity_from_position: s.velocity_from_position,
            velocity: s.velocity,
            notes: s.audition_notes,
            length_ms: s.audition_length_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    /// `editor`, `modulation` or `librarian`.
    pub tab: String,
    /// Name of the last selected library bank (names survive a database rebuild, ids don't).
    pub bank: Option<String>,
    /// F7: also select the loaded library patch on the synth.
    pub select_on_synth: bool,
    /// The Store dialog fetches the synth's edit buffer before storing (other controllers may
    /// have changed it behind the editor's back).
    pub fetch_before_store: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self { tab: "editor".to_owned(), bank: None, select_on_synth: false, fetch_before_store: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderPrefs {
    pub import: Option<String>,
    pub export: Option<String>,
}

impl FolderPrefs {
    /// A saved folder that was deleted since is forgotten.
    pub fn existing(folder: &Option<String>) -> Option<PathBuf> {
        folder.as_deref().map(PathBuf::from).filter(|p| p.is_dir())
    }

    pub fn from_path(path: &Option<PathBuf>) -> Option<String> {
        path.as_ref().map(|p| p.to_string_lossy().into_owned())
    }
}

/// Outer position and inner size in egui points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPrefs {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub maximized: bool,
}

impl Default for WindowPrefs {
    fn default() -> Self {
        Self { width: 1280.0, height: 860.0, x: None, y: None, maximized: false }
    }
}

pub const MIN_WINDOW: [f32; 2] = [1000.0, 680.0];
const MAX_WINDOW: f32 = 16_000.0;
/// Minimised windows report huge negative positions (-32000 on Windows).
const POSITION_LIMIT: f32 = 10_000.0;

impl WindowPrefs {
    fn sanitize(mut self) -> Option<Self> {
        if !self.width.is_finite() || !self.height.is_finite() {
            return None;
        }
        self.width = self.width.clamp(MIN_WINDOW[0], MAX_WINDOW);
        self.height = self.height.clamp(MIN_WINDOW[1], MAX_WINDOW);
        let sane = |v: Option<f32>| v.filter(|v| v.is_finite() && v.abs() <= POSITION_LIMIT);
        let (x, y) = (sane(self.x), sane(self.y));
        // Only restore a complete position.
        (self.x, self.y) = if x.is_some() && y.is_some() { (x, y) } else { (None, None) };
        Some(self)
    }

    /// The window as eframe reports it. A minimised, fullscreen or unmeasured window keeps the
    /// `previous` geometry; a maximised one keeps its restored size and position.
    pub fn capture(info: &egui::ViewportInfo, previous: Option<Self>) -> Option<Self> {
        if info.minimized == Some(true) || info.fullscreen == Some(true) {
            return previous;
        }
        let maximized = info.maximized == Some(true);
        if maximized {
            return previous.or(Some(Self::default())).map(|p| Self { maximized, ..p });
        }
        let (Some(inner), outer) = (info.inner_rect, info.outer_rect) else {
            return previous;
        };
        Self {
            width: inner.width(),
            height: inner.height(),
            x: outer.map(|r| r.min.x),
            y: outer.map(|r| r.min.y),
            maximized,
        }
        .sanitize()
        .or(previous)
    }

    pub fn apply(&self, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let builder = builder.with_inner_size([self.width, self.height]).with_maximized(self.maximized);
        match (self.x, self.y) {
            (Some(x), Some(y)) => builder.with_position([x, y]),
            _ => builder,
        }
    }
}

impl Settings {
    /// Forces every value into its valid range, so a hand-edited file can't crash the app.
    pub fn sanitize(&mut self) {
        let m = &mut self.midi;
        m.channel = m.channel.min(15);
        if m.device_id > DEVICE_ID_OMNI {
            m.device_id = MidiPrefs::default().device_id;
        }
        if m.part != PART_SINGLE_BUFFER && m.part > 15 {
            m.part = PART_SINGLE_BUFFER;
        }
        m.input = m.input.take().filter(|n| !n.is_empty());
        m.output = m.output.take().filter(|n| !n.is_empty());

        let a = &mut self.audition;
        a.octave = a.octave.clamp(MIN_OCTAVE, MAX_OCTAVE);
        a.velocity = a.velocity.clamp(1, 127);
        a.notes.retain(|n| *n <= 127);
        a.notes.truncate(8);
        if a.notes.is_empty() {
            a.notes = AuditionPrefs::default().notes;
        }
        a.length_ms = a.length_ms.clamp(50, 60_000);

        if !matches!(self.ui.tab.as_str(), "editor" | "modulation" | "librarian") {
            self.ui.tab = UiPrefs::default().tab;
        }
        self.window = self.window.and_then(WindowPrefs::sanitize);
        self.version = CURRENT_VERSION;
    }

    /// The edited part as an [`EditPart`].
    pub fn edit_part(&self) -> EditPart {
        part_from_raw(self.midi.part)
    }
}

pub fn part_from_raw(raw: u8) -> EditPart {
    if raw <= 15 {
        EditPart::Multi(raw)
    } else {
        EditPart::Single
    }
}

/// Upgrades an older file's JSON to [`CURRENT_VERSION`], one step per version.
/// A missing version is 0 (a file written before versioning) and has the v1 layout.
fn migrate(root: &mut Map<String, Value>) {
    let mut version = root.get("version").and_then(Value::as_u64).unwrap_or(0);
    while version < u64::from(CURRENT_VERSION) {
        // Add one arm per future format change, e.g. `1 => rename_fields(root)`.
        version += 1;
    }
    if version == u64::from(CURRENT_VERSION) {
        root.insert("version".to_owned(), Value::from(CURRENT_VERSION));
    }
}

/// Parses a settings file. Unknown fields are ignored, missing or malformed sections take
/// their defaults; only text that is not a JSON object is an error.
pub fn parse(text: &str) -> Result<Settings, String> {
    let Value::Object(mut root) = serde_json::from_str::<Value>(text).map_err(|e| e.to_string())? else {
        return Err("the file does not contain a JSON object".to_owned());
    };
    migrate(&mut root);
    fn section<T: serde::de::DeserializeOwned + Default>(root: &Map<String, Value>, key: &str) -> T {
        root.get(key).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }
    let mut settings = Settings {
        version: CURRENT_VERSION,
        midi: section(&root, "midi"),
        audition: section(&root, "audition"),
        ui: section(&root, "ui"),
        folders: section(&root, "folders"),
        window: section::<Option<WindowPrefs>>(&root, "window"),
    };
    settings.sanitize();
    Ok(settings)
}

pub fn serialize(settings: &Settings) -> String {
    serde_json::to_string_pretty(settings).expect("settings always serialise")
}

/// What loading found.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    /// Something the user should see (a corrupt file was set aside, an unreadable file).
    pub warning: Option<String>,
}

/// Reads `path`. A missing file gives the defaults silently; a corrupt one is renamed to
/// `<name>.bak` (replacing an older backup) and the defaults are used with a warning.
pub fn load_from(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded { settings: Settings::default(), warning: None };
        }
        Err(e) => {
            return Loaded {
                settings: Settings::default(),
                warning: Some(format!("Settings could not be read ({}): {e}. Using defaults.", path.display())),
            };
        }
    };
    match parse(&text) {
        Ok(settings) => Loaded { settings, warning: None },
        Err(reason) => {
            let backup = backup_path(path);
            let moved = std::fs::rename(path, &backup);
            let warning = match moved {
                Ok(()) => format!(
                    "The settings file was corrupt ({reason}); it was renamed to {} and defaults are used.",
                    backup.display()
                ),
                Err(e) => format!("The settings file is corrupt ({reason}) and could not be renamed: {e}. Using defaults."),
            };
            Loaded { settings: Settings::default(), warning: Some(warning) }
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(std::ffi::OsString::from).unwrap_or_default();
    name.push(".bak");
    path.with_file_name(name)
}

/// Writes via a temporary file so a crash never leaves half a settings file.
pub fn save_to(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut tmp_name = path.file_name().map(std::ffi::OsString::from).unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, serialize(settings)).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

/// Remembers what is on disk and writes changes, rate limited.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: Option<PathBuf>,
    saved: Settings,
    last_write: Option<Instant>,
}

impl SettingsStore {
    /// Loads from the user data directory (defaults if there is none).
    pub fn load() -> (Self, Loaded) {
        match data_dir() {
            Ok(dir) => Self::load_at(dir.join(SETTINGS_FILE_NAME)),
            Err(_) => (Self { path: None, saved: Settings::default(), last_write: None }, Loaded {
                settings: Settings::default(),
                warning: None,
            }),
        }
    }

    pub fn load_at(path: PathBuf) -> (Self, Loaded) {
        let loaded = load_from(&path);
        (Self { path: Some(path), saved: loaded.settings.clone(), last_write: None }, loaded)
    }

    /// `true` while `current` differs from what was last written.
    pub fn is_dirty(&self, current: &Settings) -> bool {
        self.path.is_some() && *current != self.saved
    }

    /// Writes `current` if it differs from what was last saved and the rate limit allows it
    /// (or `force`, on exit). Returns an error text if the write failed.
    pub fn save_if_changed(&mut self, current: &Settings, now: Instant, force: bool) -> Option<String> {
        let path = self.path.as_ref()?;
        if *current == self.saved {
            return None;
        }
        if !force && self.last_write.is_some_and(|t| now.duration_since(t) < SAVE_INTERVAL) {
            return None;
        }
        self.last_write = Some(now);
        // Don't retry every frame after a failure; the next change tries again.
        self.saved = current.clone();
        save_to(path, current).err().map(|e| format!("Settings could not be saved: {e}"))
    }
}

/// Applies the saved ports once, when the first port list arrives. The saved names become the
/// wanted ports (kept even if absent, so they reconnect later); a saved port that is present or
/// renamed is connected. If one is missing, auto-detect is switched on so the Virus is still
/// found meanwhile.
pub fn restore_ports(ports: &mut Ports, prefs: &MidiPrefs, inputs: &[String], outputs: &[String]) -> Vec<MidiCommand> {
    let found = |wanted: &Option<String>, available: &[String]| {
        wanted.as_deref().is_none_or(|w| find_port_or_renamed(w, available).is_some())
    };
    let missing = !found(&prefs.input, inputs) || !found(&prefs.output, outputs);
    ports.wanted_input.clone_from(&prefs.input);
    ports.wanted_output.clone_from(&prefs.output);
    ports.auto_connect = prefs.auto_connect || missing;
    ports.on_port_list(inputs.to_vec(), outputs.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("virus-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn round_trips() {
        let mut s = Settings {
            midi: MidiPrefs {
                input: Some("Virus TI MIDI In".into()),
                output: Some("Virus TI MIDI Out".into()),
                auto_connect: false,
                channel: 4,
                device_id: DEVICE_ID_OMNI,
                part: 2,
            },
            ui: UiPrefs {
                tab: "librarian".into(),
                bank: Some("Bank B".into()),
                select_on_synth: true,
                fetch_before_store: true,
            },
            window: Some(WindowPrefs { width: 1400.0, height: 900.0, x: Some(10.0), y: Some(20.0), maximized: false }),
            ..Settings::default()
        };
        s.audition.octave = -2;
        s.audition.velocity_from_position = false;
        s.audition.notes = vec![48, 52, 55];
        s.folders.import = Some("C:\\Patches".into());
        assert_eq!(parse(&serialize(&s)).unwrap(), s);
    }

    #[test]
    fn defaults_keep_hold_off_and_select_off() {
        let s = Settings::default();
        assert!(!s.ui.select_on_synth);
        assert!(!s.ui.fetch_before_store, "fetching before a store is opt-in");
        assert!(!serialize(&s).contains("latch") && !serialize(&s).contains("hold"));
        assert!(s.midi.auto_connect);
    }

    #[test]
    fn unknown_old_and_malformed_fields_fall_back_to_defaults() {
        let text = r#"{"midi":{"channel":3,"future_option":true},"audition":"nonsense",
                       "brand_new_section":{"a":1},"ui":{"tab":"librarian"}}"#;
        let s = parse(text).unwrap();
        assert_eq!(s.midi.channel, 3);
        assert_eq!(s.midi.device_id, 0);
        assert_eq!(s.audition, AuditionPrefs::default());
        assert_eq!(s.ui.tab, "librarian");
        assert_eq!(s.version, CURRENT_VERSION);
    }

    #[test]
    fn unversioned_and_newer_files_load() {
        assert_eq!(parse("{}").unwrap(), Settings::default());
        let newer = parse(r#"{"version":99,"midi":{"channel":7}}"#).unwrap();
        assert_eq!(newer.midi.channel, 7);
        assert_eq!(newer.version, CURRENT_VERSION);
    }

    #[test]
    fn migrate_stamps_the_current_version() {
        let mut root = Map::new();
        migrate(&mut root);
        assert_eq!(root["version"], Value::from(CURRENT_VERSION));
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let text = r#"{"midi":{"channel":99,"device_id":50,"part":30},
            "audition":{"octave":40,"velocity":0,"notes":[200],"length_ms":1},
            "ui":{"tab":"bogus"},
            "window":{"width":5,"height":1e9,"x":-32000,"y":10}}"#;
        let s = parse(text).unwrap();
        assert_eq!((s.midi.channel, s.midi.device_id, s.midi.part), (15, 0, PART_SINGLE_BUFFER));
        assert_eq!(s.audition.octave, MAX_OCTAVE);
        assert_eq!(s.audition.velocity, 1);
        assert_eq!(s.audition.notes, vec![60]);
        assert_eq!(s.audition.length_ms, 50);
        assert_eq!(s.ui.tab, "editor");
        let w = s.window.unwrap();
        assert_eq!((w.width, w.height, w.x, w.y), (MIN_WINDOW[0], MAX_WINDOW, None, None));
    }

    #[test]
    fn part_mapping() {
        assert_eq!(part_from_raw(PART_SINGLE_BUFFER), EditPart::Single);
        assert_eq!(part_from_raw(0), EditPart::Multi(0));
        assert_eq!(part_from_raw(15), EditPart::Multi(15));
        assert_eq!(part_from_raw(16), EditPart::Single);
    }

    #[test]
    fn corrupt_file_is_moved_aside_with_a_warning() {
        let dir = temp_dir("settings-corrupt");
        let path = dir.join(SETTINGS_FILE_NAME);
        std::fs::write(&path, "{ not json").unwrap();
        std::fs::write(dir.join("settings.json.bak"), "older backup").unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.warning.unwrap().contains(".bak"));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(dir.join("settings.json.bak")).unwrap(), "{ not json");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_gives_defaults_silently() {
        let dir = temp_dir("settings-missing");
        let loaded = load_from(&dir.join(SETTINGS_FILE_NAME));
        assert_eq!(loaded, Loaded { settings: Settings::default(), warning: None });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn store_saves_changes_rate_limited_and_forced() {
        let dir = temp_dir("settings-store");
        let path = dir.join("nested").join(SETTINGS_FILE_NAME);
        let (mut store, loaded) = SettingsStore::load_at(path.clone());
        let t0 = Instant::now();
        let mut s = loaded.settings;
        assert_eq!(store.save_if_changed(&s, t0, false), None);
        assert!(!path.exists(), "unchanged settings are not written");

        s.midi.channel = 5;
        assert_eq!(store.save_if_changed(&s, t0, false), None);
        assert_eq!(load_from(&path).settings.midi.channel, 5);

        s.midi.channel = 6;
        store.save_if_changed(&s, t0 + Duration::from_millis(100), false);
        assert_eq!(load_from(&path).settings.midi.channel, 5, "rate limited");
        store.save_if_changed(&s, t0 + Duration::from_millis(100), true);
        assert_eq!(load_from(&path).settings.midi.channel, 6, "forced on exit");
        assert!(!dir.join("nested").join("settings.json.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_ports_that_exist_are_selected() {
        let prefs = MidiPrefs {
            input: Some("Virus In".into()),
            output: Some("Virus Out".into()),
            auto_connect: false,
            ..MidiPrefs::default()
        };
        let mut ports = Ports::default();
        let commands = restore_ports(&mut ports, &prefs, &names(&["Other", "Virus In"]), &names(&["Virus Out"]));
        assert_eq!(commands, vec![MidiCommand::ConnectInput("Virus In".into()), MidiCommand::ConnectOutput("Virus Out".into())]);
        assert_eq!(ports.selected_input.as_deref(), Some("Virus In"));
        assert!(!ports.auto_connect);
    }

    #[test]
    fn vanished_ports_are_remembered_and_auto_detect_stands_in() {
        let prefs = MidiPrefs {
            input: Some("Gone In".into()),
            output: Some("Virus Out".into()),
            auto_connect: false,
            ..MidiPrefs::default()
        };
        let mut ports = Ports::default();
        let commands = restore_ports(&mut ports, &prefs, &names(&["Virus In"]), &names(&["Virus Out"]));
        assert_eq!(commands, vec![MidiCommand::ConnectInput("Virus In".into()), MidiCommand::ConnectOutput("Virus Out".into())]);
        assert_eq!(ports.selected_input, Some("Virus In".into()));
        assert!(ports.auto_connect);
        let more = ports.on_port_list(names(&["Virus In"]), names(&["Virus Out"]));
        assert!(more.is_empty());
        assert_eq!(ports.wanted_input.as_deref(), Some("Gone In"), "the saved name is remembered");
    }

    #[test]
    fn saved_port_renamed_by_windows_is_connected_without_auto_detect() {
        let prefs = MidiPrefs {
            output: Some("mioXC MIDI OUT".into()),
            auto_connect: false,
            ..MidiPrefs::default()
        };
        let mut ports = Ports::default();
        let commands = restore_ports(&mut ports, &prefs, &[], &names(&["2- mioXC MIDI OUT"]));
        assert_eq!(commands, vec![MidiCommand::ConnectOutput("2- mioXC MIDI OUT".into())]);
        assert!(!ports.auto_connect);
        assert_eq!(ports.wanted_output.as_deref(), Some("mioXC MIDI OUT"));
    }

    #[test]
    fn deliberately_disconnected_stays_disconnected() {
        let prefs = MidiPrefs { auto_connect: false, ..MidiPrefs::default() };
        let mut ports = Ports::default();
        assert!(restore_ports(&mut ports, &prefs, &names(&["Virus In"]), &names(&["Virus Out"])).is_empty());
        assert!(ports.on_port_list(names(&["Virus In"]), names(&["Virus Out"])).is_empty());
    }

    fn info(inner: Option<(f32, f32)>, outer_min: (f32, f32)) -> egui::ViewportInfo {
        let rect = |w: f32, h: f32, p: (f32, f32)| egui::Rect::from_min_size(egui::pos2(p.0, p.1), egui::vec2(w, h));
        egui::ViewportInfo {
            inner_rect: inner.map(|(w, h)| rect(w, h, (outer_min.0 + 8.0, outer_min.1 + 30.0))),
            outer_rect: inner.map(|(w, h)| rect(w + 16.0, h + 38.0, outer_min)),
            ..Default::default()
        }
    }

    #[test]
    fn window_capture_uses_inner_size_and_outer_position() {
        let w = WindowPrefs::capture(&info(Some((1300.0, 800.0)), (50.0, 60.0)), None).unwrap();
        assert_eq!((w.width, w.height, w.x, w.y, w.maximized), (1300.0, 800.0, Some(50.0), Some(60.0), false));
    }

    #[test]
    fn window_capture_ignores_minimised_and_keeps_restored_size_when_maximised() {
        let previous = WindowPrefs { width: 1100.0, height: 700.0, x: Some(5.0), y: Some(6.0), maximized: false };
        let mut minimised = info(Some((160.0, 28.0)), (-32000.0, -32000.0));
        minimised.minimized = Some(true);
        assert_eq!(WindowPrefs::capture(&minimised, Some(previous)), Some(previous));

        let mut maximised = info(Some((1920.0, 1000.0)), (0.0, 0.0));
        maximised.maximized = Some(true);
        let w = WindowPrefs::capture(&maximised, Some(previous)).unwrap();
        assert_eq!((w.width, w.height, w.maximized), (1100.0, 700.0, true));

        assert_eq!(WindowPrefs::capture(&info(None, (0.0, 0.0)), Some(previous)), Some(previous));
    }
}
