//! File dialogs and file I/O off the UI thread (U5).
//!
//! The UI thread calls [`FileJobs::start`] with a [`Task`]; a worker thread runs it (an `rfd`
//! dialog or a file read/write) and sends the [`Outcome`] back, and the UI thread picks it up
//! with [`FileJobs::poll`] each frame. Only one task runs at a time, which also keeps a single
//! dialog open. The database is never touched here: callers apply outcomes on the UI thread.

use std::future::Future;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    PickImport { dir: Option<PathBuf> },
    PickExport { dir: Option<PathBuf>, file_name: String },
    ReadFile(PathBuf),
    WriteFile { path: PathBuf, bytes: Vec<u8> },
}

impl Task {
    /// Shown next to the busy indicator.
    pub fn label(&self) -> &'static str {
        match self {
            Task::PickImport { .. } | Task::PickExport { .. } => "Waiting for the file dialog…",
            Task::ReadFile(_) => "Reading the file…",
            Task::WriteFile { .. } => "Writing the file…",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `None` = the dialog was cancelled.
    ImportPicked(Option<PathBuf>),
    ExportPicked(Option<PathBuf>),
    Read { path: PathBuf, result: Result<Vec<u8>, String> },
    Written { path: PathBuf, result: Result<(), String> },
    /// The worker panicked.
    Failed(String),
}

/// Runs a task to completion; replaced by a fake in tests.
pub trait Backend: Send + Sync {
    fn run(&self, task: Task) -> Outcome;
}

/// The real dialogs (`rfd::AsyncFileDialog`, polled on the worker thread) and `std::fs`.
pub struct SystemBackend;

impl Backend for SystemBackend {
    fn run(&self, task: Task) -> Outcome {
        match task {
            Task::PickImport { dir } => {
                let mut dialog = rfd::AsyncFileDialog::new()
                    .add_filter("SysEx / MIDI", &["syx", "mid", "midi", "bin"])
                    .add_filter("All files", &["*"]);
                if let Some(dir) = dir {
                    dialog = dialog.set_directory(dir);
                }
                Outcome::ImportPicked(block_on(dialog.pick_file()).map(|f| f.path().to_path_buf()))
            }
            Task::PickExport { dir, file_name } => {
                let mut dialog = rfd::AsyncFileDialog::new().add_filter("SysEx", &["syx"]).set_file_name(file_name);
                if let Some(dir) = dir {
                    dialog = dialog.set_directory(dir);
                }
                Outcome::ExportPicked(block_on(dialog.save_file()).map(|f| f.path().to_path_buf()))
            }
            Task::ReadFile(path) => {
                let result = std::fs::read(&path).map_err(|e| format!("Cannot read {}: {e}", path.display()));
                Outcome::Read { path, result }
            }
            Task::WriteFile { path, bytes } => {
                let result = std::fs::write(&path, bytes).map_err(|e| format!("Cannot write {}: {e}", path.display()));
                Outcome::Written { path, result }
            }
        }
    }
}

struct ThreadWaker(thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Minimal executor: parks the worker thread until the future wakes it.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}

/// A task was started while another one was still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

pub struct FileJobs {
    backend: Arc<dyn Backend>,
    sender: Sender<Outcome>,
    receiver: Receiver<Outcome>,
    busy: Option<&'static str>,
    /// Folders of the last import / export, offered when the next dialog opens.
    pub import_dir: Option<PathBuf>,
    pub export_dir: Option<PathBuf>,
}

impl Default for FileJobs {
    fn default() -> Self {
        Self::with_backend(Arc::new(SystemBackend))
    }
}

impl FileJobs {
    pub fn with_backend(backend: Arc<dyn Backend>) -> Self {
        let (sender, receiver) = channel();
        Self { backend, sender, receiver, busy: None, import_dir: None, export_dir: None }
    }

    /// What is running, for the busy indicator; `None` when idle.
    pub fn busy(&self) -> Option<&'static str> {
        self.busy
    }

    /// Runs `task` on a worker thread, then calls `wake` (to repaint the UI).
    pub fn start(&mut self, task: Task, wake: impl Fn() + Send + 'static) -> Result<(), Busy> {
        if self.busy.is_some() {
            return Err(Busy);
        }
        self.busy = Some(task.label());
        let backend = Arc::clone(&self.backend);
        let sender = self.sender.clone();
        thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.run(task)))
                .unwrap_or_else(|_| Outcome::Failed("the file worker crashed".to_owned()));
            let _ = sender.send(outcome);
            wake();
        });
        Ok(())
    }

    /// The finished task's outcome, if any. Call on the UI thread every frame.
    pub fn poll(&mut self) -> Option<Outcome> {
        let outcome = self.receiver.try_recv().ok()?;
        self.busy = None;
        Some(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Blocks `PickImport` until released, so tests can observe the busy state.
    struct Fake {
        gate: Mutex<Receiver<()>>,
    }

    impl Backend for Fake {
        fn run(&self, task: Task) -> Outcome {
            match task {
                Task::PickImport { dir } => {
                    self.gate.lock().unwrap().recv().unwrap();
                    Outcome::ImportPicked(dir.map(|d| d.join("bank.syx")))
                }
                Task::ReadFile(path) => Outcome::Read { path, result: Ok(vec![1, 2, 3]) },
                Task::PickExport { .. } => panic!("boom"),
                Task::WriteFile { path, .. } => Outcome::Written { path, result: Err("disk full".into()) },
            }
        }
    }

    fn jobs() -> (FileJobs, Sender<()>) {
        let (release, gate) = channel();
        (FileJobs::with_backend(Arc::new(Fake { gate: Mutex::new(gate) })), release)
    }

    fn wait(jobs: &mut FileJobs) -> Outcome {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(outcome) = jobs.poll() {
                return outcome;
            }
            assert!(Instant::now() < deadline, "no outcome");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn runs_off_thread_reports_busy_and_rejects_a_second_task() {
        let (mut jobs, release) = jobs();
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        jobs.start(Task::PickImport { dir: Some(PathBuf::from("songs")) }, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        assert!(jobs.busy().is_some());
        assert_eq!(jobs.poll(), None, "the dialog is still open");
        assert_eq!(jobs.start(Task::ReadFile("x".into()), || {}), Err(Busy));

        release.send(()).unwrap();
        assert_eq!(wait(&mut jobs), Outcome::ImportPicked(Some(PathBuf::from("songs").join("bank.syx"))));
        assert_eq!(jobs.busy(), None);
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn tasks_chain_and_failures_come_back_as_outcomes() {
        let (mut jobs, _release) = jobs();
        jobs.start(Task::ReadFile("a.syx".into()), || {}).unwrap();
        assert_eq!(wait(&mut jobs), Outcome::Read { path: "a.syx".into(), result: Ok(vec![1, 2, 3]) });
        jobs.start(Task::WriteFile { path: "b.syx".into(), bytes: vec![] }, || {}).unwrap();
        assert_eq!(wait(&mut jobs), Outcome::Written { path: "b.syx".into(), result: Err("disk full".into()) });
        jobs.start(Task::PickExport { dir: None, file_name: "x.syx".into() }, || {}).unwrap();
        assert!(matches!(wait(&mut jobs), Outcome::Failed(_)), "a worker panic must not leave the UI busy");
        assert_eq!(jobs.busy(), None);
    }

    #[test]
    fn system_backend_reads_and_writes_files() {
        let path = std::env::temp_dir().join(format!("virus-app-dialogs-{}.syx", std::process::id()));
        let write = SystemBackend.run(Task::WriteFile { path: path.clone(), bytes: vec![0xF0, 0xF7] });
        assert_eq!(write, Outcome::Written { path: path.clone(), result: Ok(()) });
        let read = SystemBackend.run(Task::ReadFile(path.clone()));
        assert_eq!(read, Outcome::Read { path: path.clone(), result: Ok(vec![0xF0, 0xF7]) });
        std::fs::remove_file(&path).unwrap();
        let Outcome::Read { result, .. } = SystemBackend.run(Task::ReadFile(path)) else { panic!() };
        assert!(result.unwrap_err().starts_with("Cannot read"));
    }

    #[test]
    fn block_on_waits_for_a_wake() {
        struct YieldOnce(bool);
        impl Future for YieldOnce {
            type Output = u8;
            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
                if self.0 {
                    return Poll::Ready(7);
                }
                self.0 = true;
                let waker = cx.waker().clone();
                thread::spawn(move || waker.wake());
                Poll::Pending
            }
        }
        assert_eq!(block_on(YieldOnce(false)), 7);
    }
}
