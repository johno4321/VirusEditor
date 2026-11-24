//! UI panels. They only read the patch and the database and report what the user did;
//! the app applies edits through `Session` so every change is also sent to the synth.

pub mod comforts;
pub mod controls;
pub mod editor;
pub mod librarian;
pub mod midi_bar;
pub mod modulation;

use egui::Color32;
use virus_syx::Page;

/// A change the user made to the edited patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Param { page: Page, number: u8, value: u8 },
    Name(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

/// A message for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub level: Level,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Self { text: text.into(), level: Level::Info }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self { text: text.into(), level: Level::Warning }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), level: Level::Error }
    }

    pub fn colour(&self) -> Color32 {
        match self.level {
            Level::Info => crate::theme::DIM,
            Level::Warning => crate::theme::WARN,
            Level::Error => crate::theme::DANGER,
        }
    }
}

pub const DIM: Color32 = crate::theme::DIM;
pub const ACCENT: Color32 = crate::theme::ACCENT_TEXT;
pub const WARN: Color32 = crate::theme::WARN;
pub const DANGER: Color32 = crate::theme::DANGER;
