//! Librarian file import/export on top of [`FileJobs`]: dialogs and file I/O run on a worker
//! thread, while the database (`import_bytes`, `export_bank_bytes`) stays on the UI thread.

use std::path::{Path, PathBuf};

use egui::Context;
use virus_db::{ImportTarget, PatchDatabase};

use super::{LibAction, LibrarianView};
use crate::dialogs::{Outcome, Task};
use crate::library::{export_bank_bytes, import_bytes};
use crate::views::Notice;

/// What the running file task is for, captured when the user clicked.
pub(super) enum PendingFile {
    Import { target: ImportTarget },
    Export { bank_id: i64, bank_name: String, export_as: Option<u8>, count: usize },
}

impl LibrarianView {
    pub(super) fn start_import(&mut self, ctx: &Context, actions: &mut Vec<LibAction>) {
        let target = match (self.import_into_selected, self.query.bank_id) {
            (true, Some(id)) => ImportTarget::Bank(id),
            _ => ImportTarget::OriginalBanks,
        };
        if self.start_task(ctx, Task::PickImport { dir: self.files.import_dir.clone() }, actions) {
            self.pending_file = Some(PendingFile::Import { target });
        }
    }

    pub(super) fn start_export(&mut self, ctx: &Context, actions: &mut Vec<LibAction>) {
        let Some(bank) = self.query.bank_id.and_then(|id| self.cache.bank(id)).cloned() else {
            actions.push(LibAction::Notice(Notice::warning("Select a bank to export.")));
            return;
        };
        let file_name: String = bank.name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let task = Task::PickExport { dir: self.files.export_dir.clone(), file_name: format!("{file_name}.syx") };
        if self.start_task(ctx, task, actions) {
            self.pending_file =
                Some(PendingFile::Export { bank_id: bank.id, bank_name: bank.name, export_as: self.export_as, count: 0 });
        }
    }

    fn start_task(&mut self, ctx: &Context, task: Task, actions: &mut Vec<LibAction>) -> bool {
        let label = task.label();
        let ctx = ctx.clone();
        match self.files.start(task, move || ctx.request_repaint()) {
            Ok(()) => {
                actions.push(LibAction::Notice(Notice::info(label)));
                true
            }
            Err(_) => false,
        }
    }

    /// Applies a finished file task on the UI thread. Call every frame.
    pub(super) fn poll_files(&mut self, ctx: &Context, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        let Some(outcome) = self.files.poll() else { return };
        let Some(pending) = self.pending_file.take() else { return };
        match (pending, outcome) {
            (_, Outcome::Failed(e)) => actions.push(LibAction::Notice(Notice::error(e))),
            (pending @ PendingFile::Import { .. }, Outcome::ImportPicked(Some(path))) => {
                self.files.import_dir = parent_of(&path);
                if self.start_task(ctx, Task::ReadFile(path), actions) {
                    self.pending_file = Some(pending);
                }
            }
            (PendingFile::Import { target }, Outcome::Read { path, result }) => {
                match result.and_then(|bytes| import_bytes(db, &bytes, target)) {
                    Ok(outcome) => {
                        let file = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                        let text = format!("{file}: {}", outcome.summary());
                        actions.push(LibAction::Notice(if outcome.has_warnings() { Notice::warning(text) } else { Notice::info(text) }));
                        self.loose = outcome.report.loose.clone();
                        self.last_import = Some(outcome);
                    }
                    Err(e) => actions.push(LibAction::Notice(Notice::error(e))),
                }
            }
            (PendingFile::Export { bank_id, bank_name, export_as, .. }, Outcome::ExportPicked(Some(path))) => {
                self.files.export_dir = parent_of(&path);
                match export_bank_bytes(db, bank_id, export_as) {
                    Ok((bytes, count)) => {
                        if self.start_task(ctx, Task::WriteFile { path, bytes }, actions) {
                            self.pending_file = Some(PendingFile::Export { bank_id, bank_name, export_as, count });
                        }
                    }
                    Err(e) => actions.push(LibAction::Notice(Notice::error(e))),
                }
            }
            (PendingFile::Export { bank_name, count, .. }, Outcome::Written { path, result }) => {
                actions.push(LibAction::Notice(match result {
                    Ok(()) => Notice::info(format!("Exported {count} patch(es) of {bank_name} to {}.", path.display())),
                    Err(e) => Notice::error(e),
                }));
            }
            // A cancelled dialog, or an outcome that doesn't belong to the pending request.
            _ => {}
        }
    }
}

fn parent_of(path: &Path) -> Option<PathBuf> {
    path.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use egui::{CentralPanel, RawInput};
    use virus_syx::{encode_program_dump, Patch};

    use super::*;
    use crate::dialogs::{Backend, FileJobs};

    struct Fake {
        file: Vec<u8>,
        written: Mutex<Vec<u8>>,
    }

    impl Backend for Fake {
        fn run(&self, task: Task) -> Outcome {
            match task {
                Task::PickImport { dir } => {
                    assert_eq!(dir, Some(PathBuf::from("remembered")));
                    Outcome::ImportPicked(Some(PathBuf::from("songs").join("in.syx")))
                }
                Task::ReadFile(path) => Outcome::Read { path, result: Ok(self.file.clone()) },
                Task::PickExport { file_name, .. } => {
                    assert!(file_name.ends_with(".syx"));
                    Outcome::ExportPicked(Some(PathBuf::from("out").join(file_name)))
                }
                Task::WriteFile { path, bytes } => {
                    *self.written.lock().unwrap() = bytes;
                    Outcome::Written { path, result: Ok(()) }
                }
            }
        }
    }

    fn pump(view: &mut LibrarianView, ctx: &Context, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while view.files.busy().is_some() || view.pending_file.is_some() {
            view.poll_files(ctx, db, actions);
            assert!(Instant::now() < deadline, "file task never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn texts(actions: &[LibAction]) -> Vec<String> {
        actions
            .iter()
            .filter_map(|a| match a {
                LibAction::Notice(n) => Some(n.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn import_then_export_run_through_the_worker_and_remember_folders() {
        let file = encode_program_dump(0, 1, 5, Patch::init().as_bytes());
        let fake = Arc::new(Fake { file, written: Mutex::new(Vec::new()) });
        let mut view = LibrarianView { files: FileJobs::with_backend(fake.clone()), ..Default::default() };
        view.files.import_dir = Some(PathBuf::from("remembered"));
        let db = PatchDatabase::open_in_memory().unwrap();
        let ctx = Context::default();
        let mut actions = Vec::new();

        view.start_import(&ctx, &mut actions);
        assert!(view.files.busy().is_some());
        // A second click while the dialog is open starts nothing.
        let before = actions.len();
        view.start_import(&ctx, &mut actions);
        assert_eq!(actions.len(), before);
        pump(&mut view, &ctx, &db, &mut actions);
        assert!(view.last_import.is_some(), "{:?}", texts(&actions));
        assert_eq!(view.files.import_dir, Some(PathBuf::from("songs")));

        view.refresh(&db);
        view.query.bank_id = view.cache.banks.first().map(|b| b.id);
        let bank = db.get_banks().unwrap().into_iter().find(|b| b.bank_letter.as_deref() == Some("A")).unwrap();
        view.query.bank_id = Some(bank.id);
        view.refresh(&db);
        view.start_export(&ctx, &mut actions);
        pump(&mut view, &ctx, &db, &mut actions);
        assert!(!fake.written.lock().unwrap().is_empty());
        assert_eq!(view.files.export_dir, Some(PathBuf::from("out")));
        assert!(texts(&actions).last().unwrap().starts_with("Exported 1 patch(es)"), "{:?}", texts(&actions));
    }

    #[test]
    fn unreadable_file_is_reported() {
        struct Failing;
        impl Backend for Failing {
            fn run(&self, task: Task) -> Outcome {
                match task {
                    Task::ReadFile(path) => Outcome::Read { path, result: Err("Cannot read: gone".into()) },
                    _ => Outcome::ImportPicked(Some("x.syx".into())),
                }
            }
        }
        let mut view = LibrarianView { files: FileJobs::with_backend(Arc::new(Failing)), ..Default::default() };
        let db = PatchDatabase::open_in_memory().unwrap();
        let ctx = Context::default();
        let mut actions = Vec::new();
        view.start_import(&ctx, &mut actions);
        pump(&mut view, &ctx, &db, &mut actions);
        assert_eq!(texts(&actions).last().unwrap(), "Cannot read: gone");
    }

    #[test]
    fn busy_frame_renders_without_panicking() {
        struct Slow(Mutex<std::sync::mpsc::Receiver<()>>);
        impl Backend for Slow {
            fn run(&self, _task: Task) -> Outcome {
                let _ = self.0.lock().unwrap().recv();
                Outcome::ImportPicked(None)
            }
        }
        let (release, gate) = std::sync::mpsc::channel();
        let mut view = LibrarianView { files: FileJobs::with_backend(Arc::new(Slow(Mutex::new(gate)))), ..Default::default() };
        let db = PatchDatabase::open_in_memory().unwrap();
        let ctx = Context::default();
        let mut actions = Vec::new();
        let mut received = Vec::new();
        view.start_import(&ctx, &mut actions);
        for _ in 0..2 {
            let _ = ctx.run(RawInput::default(), |ctx| {
                CentralPanel::default().show(ctx, |ui| view.show(ui, &db, &mut received, &mut actions));
                view.show_dialogs(ctx, &db, &Patch::init(), &mut actions);
            });
        }
        assert!(view.files.busy().is_some());
        release.send(()).unwrap();
        pump(&mut view, &ctx, &db, &mut actions);
        assert!(view.files.busy().is_none());
    }
}
