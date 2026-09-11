//! Delayed rendering for transient interactive progress.
//!
//! Fast operations should leave no terminal artefact, while slow operations
//! must become visible promptly. Callers publish their latest status and this
//! module owns the one in-place stderr line.

use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long an interactive operation runs before its transient progress is shown.
pub const PROGRESS_REVEAL_DELAY: Duration = Duration::from_millis(500);

struct State {
    line: String,
    complete: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// A handle for publishing the latest progress line of an operation.
#[derive(Clone)]
pub struct Reporter {
    shared: Arc<Shared>,
}

impl Reporter {
    /// Replace the line that will be rendered while the operation is active.
    pub fn update(&self, line: String) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.complete {
            state.line = line;
            self.shared.changed.notify_one();
        }
    }
}

/// A delayed interactive progress line.
///
/// Dropping this value marks the operation complete and waits for the renderer
/// to clear a line that was shown. Fast operations return without writing.
pub struct Progress {
    reporter: Reporter,
    renderer: Option<JoinHandle<()>>,
}

impl Progress {
    /// Start an operation whose initial visible state is `line`.
    pub fn start(line: impl Into<String>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                line: line.into(),
                complete: false,
            }),
            changed: Condvar::new(),
        });
        let renderer_shared = Arc::clone(&shared);
        let renderer = thread::spawn(move || render(renderer_shared));
        Self {
            reporter: Reporter { shared },
            renderer: Some(renderer),
        }
    }

    /// Return a handle that may be moved to worker threads.
    pub fn reporter(&self) -> Reporter {
        self.reporter.clone()
    }

    /// Publish a new line from the operation owner.
    pub fn update(&self, line: String) {
        self.reporter.update(line);
    }

    /// Mark the operation complete and wait for the renderer to finish.
    pub fn finish(mut self) {
        self.complete();
    }

    fn complete(&mut self) {
        if let Some(renderer) = self.renderer.take() {
            let mut state = self
                .reporter
                .shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.complete = true;
            self.reporter.shared.changed.notify_one();
            drop(state);
            let _ = renderer.join();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.complete();
    }
}

fn render(shared: Arc<Shared>) {
    let started = Instant::now();
    let deadline = started + PROGRESS_REVEAL_DELAY;
    let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    while !state.complete {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let (next, timeout) = shared
            .changed
            .wait_timeout(state, remaining)
            .unwrap_or_else(|e| e.into_inner());
        state = next;
        if timeout.timed_out() {
            break;
        }
    }
    if !should_reveal(started.elapsed(), state.complete) {
        return;
    }

    let mut width = 0;
    loop {
        let line = state.line.clone();
        drop(state);
        let rendered = format!("  {line}");
        let terminal = io::stderr();
        let mut terminal = terminal.lock();
        let _ = write!(terminal, "\r{rendered:width$}");
        let _ = terminal.flush();
        width = width.max(rendered.len());

        state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        while !state.complete {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
            if !state.complete {
                break;
            }
        }
        if state.complete {
            break;
        }
    }
    drop(state);
    let terminal = io::stderr();
    let mut terminal = terminal.lock();
    let _ = write!(terminal, "\r{:width$}\r", "");
    let _ = terminal.flush();
}

fn should_reveal(elapsed: Duration, complete: bool) -> bool {
    !complete && elapsed >= PROGRESS_REVEAL_DELAY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_policy_hides_fast_operations_and_reveals_slow_ones() {
        let cases = [
            (Duration::ZERO, false, false),
            (Duration::from_millis(499), false, false),
            (Duration::from_millis(500), false, true),
            (Duration::from_secs(1), false, true),
            (Duration::from_secs(1), true, false),
        ];

        for (elapsed, complete, expected) in cases {
            assert_eq!(should_reveal(elapsed, complete), expected);
        }
    }
}
