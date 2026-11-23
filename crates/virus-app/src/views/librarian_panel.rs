//! Access Virus Patch Librarian & Bank Manager UI.

use egui::{Color32, ComboBox, Frame, Grid, RichText, ScrollArea, TextEdit, Ui};
use virus_db::{BankRecord, PatchDatabase, PatchFilter, PatchSummary, SoundCategory};
use virus_syx::domain::patch::Patch;

pub enum LibrarianAction {
    LoadPatch(Patch),
    SendPatchToEditBuffer(Patch),
    StoreActiveToSlot(u8),
    StatusMessage(String),
}

pub struct LibrarianPanel {
    pub selected_bank_id: i64,
    pub filter: PatchFilter,
    pub status_text: String,
}

impl Default for LibrarianPanel {
    fn default() -> Self {
        Self {
            selected_bank_id: 1, // Bank A by default
            filter: PatchFilter::default(),
            status_text: String::new(),
        }
    }
}

const ALL_CATEGORIES: [SoundCategory; 17] = [
    SoundCategory::Off,
    SoundCategory::Lead,
    SoundCategory::Bass,
    SoundCategory::Pad,
    SoundCategory::Decay,
    SoundCategory::Pluck,
    SoundCategory::Acid,
    SoundCategory::Classic,
    SoundCategory::Arpeggiator,
    SoundCategory::Effects,
    SoundCategory::Drums,
    SoundCategory::Percussion,
    SoundCategory::Input,
    SoundCategory::Vocoder,
    SoundCategory::Favourite1,
    SoundCategory::Favourite2,
    SoundCategory::Favourite3,
];

impl LibrarianPanel {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        db: &PatchDatabase,
        active_patch: &mut Patch,
    ) -> Option<LibrarianAction> {
        let mut triggered_action = None;

        let banks = db.get_banks().unwrap_or_default();
        if self.selected_bank_id == 0 && !banks.is_empty() {
            self.selected_bank_id = banks[0].id;
        }

        ui.vertical(|ui| {
            // --- Top Toolbar: Bank Selection & Import/Export ---
            Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("BANK:").strong().color(Color32::from_rgb(220, 70, 40)));
                    for bank in &banks {
                        let is_selected = self.selected_bank_id == bank.id;
                        let text = if let Some(letter) = &bank.bank_letter {
                            format!("Bank {}", letter)
                        } else {
                            bank.name.clone()
                        };
                        if ui.selectable_label(is_selected, text).clicked() {
                            self.selected_bank_id = bank.id;
                        }
                    }

                    ui.separator();

                    // Import Bank
                    if ui.button("Import .syx Bank...").clicked() {
                        if let Some(file_path) = rfd::FileDialog::new()
                            .add_filter("SysEx Bank File", &["syx", "bin", "mid"])
                            .pick_file()
                        {
                            if let Ok(bytes) = std::fs::read(&file_path) {
                                match db.import_syx_bank(self.selected_bank_id, &bytes) {
                                    Ok(count) => {
                                        self.status_text = format!("Successfully imported {} patches from {:?}", count, file_path.file_name().unwrap_or_default());
                                    }
                                    Err(err) => {
                                        self.status_text = format!("Import error: {}", err);
                                    }
                                }
                            }
                        }
                    }

                    // Export Bank
                    if ui.button("Export .syx Bank...").clicked() {
                        if let Some(file_path) = rfd::FileDialog::new()
                            .add_filter("SysEx Bank File", &["syx"])
                            .set_file_name("Virus_Bank.syx")
                            .save_file()
                        {
                            match db.export_syx_bank(self.selected_bank_id, 0x01) {
                                Ok(bytes) => {
                                    if std::fs::write(&file_path, bytes).is_ok() {
                                        self.status_text = format!("Bank exported successfully to {:?}", file_path.file_name().unwrap_or_default());
                                    }
                                }
                                Err(err) => {
                                    self.status_text = format!("Export error: {}", err);
                                }
                            }
                        }
                    }
                });
            });

            ui.add_space(4.0);

            // --- Filter Bar: Search, Category, Favorites ---
            Frame::canvas(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search:");
                    let mut search_str = self.filter.search_text.clone().unwrap_or_default();
                    if ui.add(TextEdit::singleline(&mut search_str).desired_width(180.0)).changed() {
                        self.filter.search_text = if search_str.trim().is_empty() {
                            None
                        } else {
                            Some(search_str)
                        };
                    }

                    ui.separator();

                    ui.label("Category:");
                    let current_cat = self.filter.category.unwrap_or(SoundCategory::Off);
                    ComboBox::from_id_source("librarian_category_filter")
                        .selected_text(current_cat.as_str())
                        .show_ui(ui, |ui| {
                            for cat in ALL_CATEGORIES {
                                let mut is_sel = current_cat == cat;
                                if ui.selectable_value(&mut is_sel, true, cat.as_str()).clicked() {
                                    self.filter.category = if cat == SoundCategory::Off { None } else { Some(cat) };
                                }
                            }
                        });

                    ui.separator();

                    ui.checkbox(&mut self.filter.favorite_only, "Favorites Only");

                    if !self.status_text.is_empty() {
                        ui.separator();
                        ui.label(RichText::new(&self.status_text).color(Color32::from_rgb(100, 220, 120)));
                    }
                });
            });

            ui.add_space(4.0);

            // --- Query and display matching patches ---
            let query_filter = PatchFilter {
                search_text: self.filter.search_text.clone(),
                bank_id: Some(self.selected_bank_id),
                category: self.filter.category,
                favorite_only: self.filter.favorite_only,
            };

            let patch_summaries = db.query_patches(&query_filter).unwrap_or_default();

            // --- Patch Table ---
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    Grid::new("patches_table")
                        .striped(true)
                        .min_col_width(70.0)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            // Header
                            ui.label(RichText::new("Prog #").strong());
                            ui.label(RichText::new("Patch Name").strong());
                            ui.label(RichText::new("Category 1").strong());
                            ui.label(RichText::new("Category 2").strong());
                            ui.label(RichText::new("Tags").strong());
                            ui.label(RichText::new("Fav").strong());
                            ui.label(RichText::new("Actions").strong());
                            ui.end_row();

                            for summary in &patch_summaries {
                                ui.label(format!("{:03}", summary.program_number));
                                ui.label(RichText::new(&summary.name).strong());
                                ui.label(summary.category1.as_str());
                                ui.label(summary.category2.as_str());
                                ui.label(
                                    RichText::new(&summary.tags)
                                        .size(11.0)
                                        .color(Color32::from_rgb(140, 145, 160)),
                                );

                                // Favorite Star
                                let fav_star = if summary.favorite { "★" } else { "☆" };
                                let fav_color = if summary.favorite {
                                    Color32::from_rgb(255, 215, 0)
                                } else {
                                    Color32::from_rgb(120, 125, 140)
                                };
                                if ui.button(RichText::new(fav_star).color(fav_color)).clicked() {
                                    let new_fav = !summary.favorite;
                                    let mut patch_clone = active_patch.clone();
                                    let _ = db.save_patch(
                                        summary.bank_id,
                                        summary.program_number,
                                        &mut patch_clone,
                                        summary.category1,
                                        summary.category2,
                                        &summary.tags,
                                        new_fav,
                                    );
                                }

                                // Actions
                                ui.horizontal(|ui| {
                                    if ui.button("Load").on_hover_text("Load patch into Editor").clicked() {
                                        if let Ok(Some(record)) = db.get_patch_by_id(summary.id) {
                                            triggered_action = Some(LibrarianAction::LoadPatch(record.patch));
                                        }
                                    }
                                    if ui.button("Send").on_hover_text("Send to Synth Single Edit Buffer").clicked() {
                                        if let Ok(Some(record)) = db.get_patch_by_id(summary.id) {
                                            triggered_action = Some(LibrarianAction::SendPatchToEditBuffer(record.patch));
                                        }
                                    }
                                    if ui.button("Store Active").on_hover_text("Overwrite this bank slot with active editor patch").clicked() {
                                        triggered_action = Some(LibrarianAction::StoreActiveToSlot(summary.program_number));
                                    }
                                });

                                ui.end_row();
                            }
                        });
                });
        });

        triggered_action
    }
}
