//! Optional observation of operations owned by wsp.
//!
//! The binary installs one observer for its invocation. Library callers have
//! no terminal side effects unless they explicitly install an observer. Event
//! delivery must be cheap and must not wait for terminal I/O.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub const PROGRESS_REVEAL_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub enum Event {
    Started {
        id: u64,
        line: String,
    },
    Updated {
        id: u64,
        line: String,
    },
    /// Authoritative measurements, separately reservable on narrow terminals.
    Measured {
        id: u64,
        line: String,
        resource: String,
        phase: String,
        detail: String,
    },
    Finished {
        id: u64,
    },
    /// Best-effort subprocess diagnostics, delivered without terminal I/O.
    Diagnostic(String),
    /// A permanent message, serialized with terminal frames by the observer.
    Message(String),
    /// Acknowledged handoff to a prompt, pager, or inherited child terminal.
    Suspended(bool),
    /// A child may access the controlling terminal; use append-only progress.
    External(bool),
}

pub trait Observer: Send + Sync {
    /// Return false to disable observation without failing the operation.
    fn observe(&self, event: Event) -> bool;
}

type Installed = Option<Arc<dyn Observer>>;
static OBSERVER: OnceLock<Mutex<Installed>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn observer_slot() -> &'static Mutex<Installed> {
    OBSERVER.get_or_init(|| Mutex::new(None))
}

/// Install an invocation observer, restoring its predecessor when dropped.
///
/// Installation is process scoped so existing core operations and scoped
/// workers can report without changing the data/JSON API or global environment.
/// Install only at an invocation boundary, not concurrently from worker threads.
pub fn install(observer: Arc<dyn Observer>) -> Installation {
    let previous = observer_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .replace(observer);
    Installation { previous }
}

pub struct Installation {
    previous: Installed,
}

impl Drop for Installation {
    fn drop(&mut self) {
        *observer_slot().lock().unwrap_or_else(|e| e.into_inner()) = self.previous.take();
    }
}

fn emit(event: Event) -> bool {
    let observer = observer_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(observer) = observer {
        if !observer.observe(event) {
            let mut slot = observer_slot().lock().unwrap_or_else(|e| e.into_inner());
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &observer))
            {
                *slot = None;
            }
        }
        true
    } else {
        false
    }
}

#[derive(Clone)]
pub struct Reporter {
    id: u64,
}

impl Reporter {
    pub fn update(&self, line: String) {
        emit(Event::Updated { id: self.id, line });
    }

    pub fn measured(&self, line: String, resource: String, phase: String, detail: String) {
        emit(Event::Measured {
            id: self.id,
            line,
            resource,
            phase,
            detail,
        });
    }
}

/// An operation scope. Completion is also published on early returns.
pub struct Progress {
    reporter: Reporter,
    complete: bool,
}

impl Progress {
    pub fn start(line: impl Into<String>) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        emit(Event::Started {
            id,
            line: line.into(),
        });
        Self {
            reporter: Reporter { id },
            complete: false,
        }
    }

    pub fn reporter(&self) -> Reporter {
        self.reporter.clone()
    }

    pub fn update(&self, line: String) {
        self.reporter.update(line);
    }

    pub fn finish(mut self) {
        self.complete();
    }

    fn complete(&mut self) {
        if !self.complete {
            self.complete = true;
            emit(Event::Finished {
                id: self.reporter.id,
            });
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.complete();
    }
}

pub fn diagnostic(line: impl Into<String>) {
    emit(Event::Diagnostic(line.into()));
}

pub fn message(line: impl fmt::Display) {
    let line = line.to_string();
    if !emit(Event::Message(line.clone())) {
        std::eprintln!("{line}");
    }
}

/// Suspend progress before displaying a prompt or handing over child streams.
pub fn suspend() -> Handoff {
    emit(Event::Suspended(true));
    Handoff { external: false }
}

/// Prevent all session frames from erasing a concurrent child's terminal input.
pub fn external() -> Handoff {
    emit(Event::External(true));
    Handoff { external: true }
}

pub struct Handoff {
    external: bool,
}

impl Drop for Handoff {
    fn drop(&mut self) {
        emit(if self.external {
            Event::External(false)
        } else {
            Event::Suspended(false)
        });
    }
}

/// Permanent stderr output coordinated with the invocation's progress line.
#[macro_export]
macro_rules! progress_message {
    () => { $crate::progress::message("") };
    ($($arg:tt)*) => { $crate::progress::message(format_args!($($arg)*)) };
}

pub use crate::progress_message as eprintln;
