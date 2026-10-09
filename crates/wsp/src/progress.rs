//! One output owner for delayed invocation progress and terminal handoffs.
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;
use wsp_core::progress::{Event, Fraction, Installation, Observer};

const DIAGNOSTICS: usize = 64;
const BAR_WIDTH: usize = 12;
const ROW_BAR_WIDTH: usize = 8;
const CURSOR_WIDTH: usize = 2;
const TICK: Duration = Duration::from_millis(100);
const MAX_WORKER_ROWS: usize = 8;

struct State {
    operations: BTreeMap<u64, String>,
    owners: BTreeMap<u64, thread::ThreadId>,
    measurements: BTreeMap<u64, Measurement>,
    worker_rows: Vec<WorkerRow>,
    invocation_thread: thread::ThreadId,
    diagnostics: usize,
    pending_output: VecDeque<String>,
    suppressed: bool,
    generation: u64,
    stopped: bool,
    suspended: usize,
    external: usize,
    external_frames: usize,
    external_lines: usize,
    elapsed: Duration,
    active_since: Option<Instant>,
    last_plain: Option<Instant>,
    last_line: String,
    frame: usize,
}

impl State {
    fn new() -> Self {
        Self {
            operations: BTreeMap::new(),
            owners: BTreeMap::new(),
            measurements: BTreeMap::new(),
            worker_rows: Vec::new(),
            invocation_thread: thread::current().id(),
            diagnostics: 0,
            pending_output: VecDeque::new(),
            suppressed: false,
            generation: 0,
            stopped: false,
            suspended: 0,
            external: 0,
            external_frames: 0,
            external_lines: 0,
            elapsed: Duration::ZERO,
            active_since: None,
            last_plain: None,
            last_line: String::new(),
            frame: 0,
        }
    }

    fn work_elapsed(&self, now: Instant) -> Duration {
        self.elapsed
            + self
                .active_since
                .map_or(Duration::ZERO, |start| now.duration_since(start))
    }

    fn account(&mut self, now: Instant) {
        if let Some(start) = self.active_since.take() {
            self.elapsed += now.duration_since(start);
        }
    }

    fn resume(&mut self, now: Instant) {
        if !self.operations.is_empty() && self.suspended == 0 && !self.stopped {
            self.active_since = Some(now);
        }
    }

    fn apply(&mut self, event: Event, now: Instant) {
        self.account(now);
        match event {
            Event::Started { id, line } => {
                let owner = thread::current().id();
                if owner != self.invocation_thread {
                    if let Some(worker) = self
                        .worker_rows
                        .iter_mut()
                        .find(|worker| worker.owner == owner)
                    {
                        if worker.complete {
                            worker.label = repository_label(&line);
                            worker.complete = false;
                        }
                    } else {
                        self.worker_rows.push(WorkerRow {
                            owner,
                            label: repository_label(&line),
                            complete: false,
                        });
                    }
                }
                self.operations.insert(id, line);
                self.owners.insert(id, owner);
            }
            Event::Updated { id, line } => {
                if let Some(current) = self.operations.get_mut(&id) {
                    *current = line;
                    self.measurements.remove(&id);
                }
            }
            Event::Measured {
                id,
                line,
                resource,
                phase,
                detail,
                fraction,
            } => {
                if let Some(current) = self.operations.get_mut(&id) {
                    *current = line;
                    self.measurements.insert(
                        id,
                        Measurement {
                            resource,
                            phase,
                            detail,
                            fraction,
                        },
                    );
                }
            }
            Event::Finished { id } => {
                self.generation += 1;
                self.operations.remove(&id);
                if let Some(owner) = self.owners.remove(&id)
                    && owner != self.invocation_thread
                    && !self
                        .owners
                        .values()
                        .any(|active_owner| *active_owner == owner)
                    && let Some(worker) = self
                        .worker_rows
                        .iter_mut()
                        .find(|worker| worker.owner == owner)
                {
                    worker.complete = true;
                }
                self.measurements.remove(&id);
                if self.operations.is_empty() {
                    self.worker_rows.clear();
                }
            }
            Event::Diagnostic(line) => {
                if self.diagnostics < DIAGNOSTICS {
                    self.diagnostics += 1;
                    self.pending_output.push_back(display_line(&line, 4096));
                } else if !self.suppressed {
                    self.suppressed = true;
                    self.pending_output
                        .push_back("Additional live diagnostics suppressed".into());
                }
            }
            Event::Suspended(value) => {
                self.generation += 1;
                if value {
                    self.suspended += 1;
                } else {
                    self.suspended = self.suspended.saturating_sub(1);
                }
            }
            Event::External(value) => {
                self.generation += 1;
                if value {
                    if self.external == 0 {
                        self.external_frames = 0;
                        self.external_lines = 0;
                        self.last_plain = None;
                        self.last_line.clear();
                    }
                    self.external += 1;
                } else {
                    self.external = self.external.saturating_sub(1);
                    if self.external == 0 {
                        self.external_frames = 0;
                        self.external_lines = 0;
                    }
                }
            }
            Event::Message(_) | Event::TerminalContext(_) | Event::Yielded(_) => {
                unreachable!("messages use the output gate")
            }
        }
        self.resume(now);
    }

    #[cfg(test)]
    fn frame(&mut self, now: Instant, tty: bool, width: usize) -> Option<Frame> {
        self.frame_with_height(now, tty, width, 24)
    }

    fn frame_with_height(
        &mut self,
        now: Instant,
        tty: bool,
        width: usize,
        height: usize,
    ) -> Option<Frame> {
        if self.stopped
            || self.suspended > 0
            || self.operations.is_empty()
            || self.work_elapsed(now) < wsp_core::progress::PROGRESS_REVEAL_DELAY
        {
            return None;
        }
        if self.external > 0 {
            let line_budget = height.saturating_sub(2);
            if line_budget <= self.external_lines || self.external_frames >= 2 {
                return None;
            }
            if self
                .last_plain
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
            {
                return None;
            }
        }
        let rotation = self.work_elapsed(now).as_secs() as usize / 2;
        // Nested scopes on one worker describe one operation. Show its deepest
        // active phase, rather than counting its ancestors as extra workers.
        let mut seen = HashSet::new();
        let mut workers = Vec::new();
        let mut summary = None;
        for (id, line) in self.operations.iter().rev() {
            let owner = self.owners[id];
            if seen.insert(owner) {
                if owner == self.invocation_thread {
                    summary = Some((*id, line));
                } else {
                    workers.push((*id, line));
                }
            }
        }
        if tty
            && !workers.is_empty()
            && workers.len().max(self.worker_rows.len()) > 1
            && width >= ROW_BAR_WIDTH + 23
            && height >= 4
        {
            let mut worker_rows = self.worker_rows.clone();
            for (id, line) in &workers {
                let owner = self.owners[id];
                if owner != self.invocation_thread
                    && !worker_rows.iter().any(|worker| worker.owner == owner)
                {
                    worker_rows.push(WorkerRow {
                        owner,
                        label: repository_label(line),
                        complete: false,
                    });
                }
            }
            let panel_height = if self.external > 0 {
                let line_budget = height.saturating_sub(2);
                let remaining_lines = line_budget.saturating_sub(self.external_lines);
                if remaining_lines < 2 {
                    return None;
                }
                let panel_lines = (line_budget / 2)
                    .max(2)
                    .min(remaining_lines)
                    .min(MAX_WORKER_ROWS);
                panel_lines + 2
            } else {
                height
            };
            let rows = worker_frame(
                WorkerFrame {
                    workers: &worker_rows,
                    operations: &self.operations,
                    owners: &self.owners,
                    measurements: &self.measurements,
                },
                self.frame,
                true,
                width,
                panel_height,
            );
            let label = rows.join("\n");
            self.last_plain = Some(now);
            self.last_line = label.clone();
            if self.external > 0 {
                self.external_frames += 1;
                self.external_lines += label.lines().count();
            }
            self.frame = self.frame.wrapping_add(1);
            return Some(Frame {
                generation: self.generation,
                text: label,
                in_place: self.external == 0,
            });
        }
        let (primary_id, primary, context, additional) = if workers.is_empty() {
            let (id, line) = summary?;
            (id, line.as_str(), None, 0)
        } else {
            let (id, line) = workers[rotation % workers.len()];
            (
                id,
                line.as_str(),
                summary.filter(|(_, line)| !line.starts_with("Running wsp")),
                workers.len() - 1,
            )
        };
        let context_measurement = context.and_then(|(id, _)| self.measurements.get(&id));
        let context = context.map(|(_, line)| line.as_str());
        let mut label = primary.to_string();
        if let Some(context) = context {
            label.push_str(&format!(" · {context}"));
        }
        if additional > 0 {
            label.push_str(&format!(" (+{additional} active)"));
        }
        if let Some(measurement) = self.measurements.get(&primary_id) {
            label.push_str(&format!(
                " · {} · {} · {}",
                measurement.resource, measurement.phase, measurement.detail
            ));
            if let Some(fraction) = measurement.fraction {
                label.push_str(&format!(" · {}/{}", fraction.completed, fraction.total));
            }
        }
        let in_place = tty && self.external == 0;
        if !in_place && let Some(last) = self.last_plain {
            let since = now.duration_since(last);
            if since < Duration::from_secs(1)
                || (label == self.last_line && since < Duration::from_secs(10))
            {
                return None;
            }
        }
        self.last_plain = Some(now);
        self.last_line = label.clone();
        if self.external > 0 {
            self.external_frames += 1;
            self.external_lines += 1;
        }
        let elapsed = self.work_elapsed(now).as_secs();
        let bar = progress_bar(
            self.measurements
                .get(&primary_id)
                .and_then(|value| value.fraction),
            self.frame,
            in_place,
        );
        self.frame = self.frame.wrapping_add(1);
        Some(Frame {
            generation: self.generation,
            text: frame_line(
                primary,
                self.measurements.get(&primary_id),
                context,
                context_measurement,
                additional,
                elapsed,
                bar.as_deref(),
                width,
            ),
            in_place,
        })
    }
}

fn take_pending_output(state: &mut State) -> Vec<String> {
    state.diagnostics = 0;
    state.suppressed = false;
    state.pending_output.drain(..).collect()
}

struct Frame {
    generation: u64,
    text: String,
    in_place: bool,
}
struct Output {
    sink: Box<dyn Write + Send>,
    visible_lines: usize,
    failed: bool,
}
impl Output {
    fn clear(&mut self) -> std::io::Result<()> {
        if self.visible_lines > 0 {
            let lines = self.visible_lines;
            self.visible_lines = 0;
            self.sink.write_all(b"\x1b[?25h")?;
            self.sink.write_all(b"\r")?;
            if lines == 1 {
                self.sink.write_all(b"\x1b[2K")?;
            } else {
                write!(self.sink, "\x1b[{}A\x1b[J", lines - 1)?;
            }
            self.sink.flush()?;
        }
        Ok(())
    }
    fn message(&mut self, text: &str) -> std::io::Result<()> {
        self.clear()?;
        writeln!(self.sink, "{text}")?;
        self.sink.flush()
    }
}

struct Renderer {
    state: Mutex<State>,
    output: Mutex<Output>,
    wake: Condvar,
    tty: bool,
}
impl Renderer {
    fn publish(&self, frame: Frame) -> bool {
        let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped || state.suspended > 0 || state.generation != frame.generation {
            return true;
        }
        // Keep validation and writing under the output gate. Handoffs take that
        // same gate before changing generation and acknowledging clearance.
        drop(state);
        if output.failed {
            return false;
        }
        let result = if frame.in_place {
            let clear = if output.visible_lines > 0 {
                output.clear()
            } else {
                output.sink.write_all(b"\r\x1b[2K")
            };
            clear.and_then(|_| {
                output.visible_lines = frame.text.lines().count().max(1);
                write!(output.sink, "\x1b[?25l{}", frame.text).and_then(|_| output.sink.flush())
            })
        } else {
            output.message(&frame.text)
        };
        output.failed = result.is_err();
        !output.failed
    }

    fn tick(&self) {
        let width = terminal_size::terminal_size_of(std::io::stderr())
            .map_or(80, |(terminal_size::Width(w), _)| usize::from(w))
            .saturating_sub(1);
        {
            let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.suspended > 0 || state.stopped {
                return;
            }
            let diagnostics = take_pending_output(&mut state);
            let idle = state.operations.is_empty();
            drop(state);
            if idle {
                output.failed |= output.clear().is_err();
            }
            for line in diagnostics {
                output.failed |= output.message(&line).is_err();
            }
        }
        let height = terminal_size::terminal_size_of(std::io::stderr())
            .map_or(24, |(terminal_size::Width(_), terminal_size::Height(h))| {
                usize::from(h)
            });
        let frame = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frame_with_height(Instant::now(), self.tty, width, height);
        if let Some(frame) = frame {
            self.publish(frame);
        }
    }

    fn stop(&self) {
        let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.account(Instant::now());
        state.stopped = true;
        state.generation += 1;
        if state.suspended == 0 {
            let diagnostics = take_pending_output(&mut state);
            drop(state);
            for line in diagnostics {
                let _ = output.message(&line);
            }
            let _ = output.clear();
        }
        self.wake.notify_all();
    }
}

impl Observer for Renderer {
    fn terminal_output(&self) -> bool {
        self.tty
    }

    fn observe(&self, event: Event) -> bool {
        match event {
            Event::TerminalContext(line) => self.observe(Event::Message(display_line(&line, 4096))),
            Event::Message(line) => {
                let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.generation += 1;
                state.pending_output.push_back(line);
                if state.suspended > 0 {
                    return true;
                }
                let pending = take_pending_output(&mut state);
                drop(state);
                if output.failed {
                    return false;
                }
                for line in pending {
                    output.failed |= output.message(&line).is_err();
                }
                !output.failed
            }
            event @ (Event::Yielded(_) | Event::Suspended(_) | Event::External(_)) => {
                let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let previously_suspended = state.suspended > 0;
                let event = match event {
                    Event::Yielded(line) => {
                        state.pending_output.push_back(display_line(&line, 4096));
                        Event::Suspended(true)
                    }
                    event => event,
                };
                state.apply(event, Instant::now());
                // Only the first handoff may write before its child starts.
                // Nested handoffs and partial resumes must leave the terminal
                // completely untouched, including permanent output.
                if previously_suspended && state.suspended > 0 {
                    return true;
                }
                let pending = take_pending_output(&mut state);
                let suspended = state.suspended > 0;
                let clear = suspended || state.external > 0 || state.operations.is_empty();
                drop(state);
                for line in pending {
                    output.failed |= output.message(&line).is_err();
                }
                if clear {
                    output.failed |= output.clear().is_err();
                }
                suspended || !output.failed
            }
            event => {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if !state.stopped {
                    state.apply(event, Instant::now());
                }
                true
            }
        }
    }
}

pub struct Session {
    renderer: Arc<Renderer>,
    worker: Option<JoinHandle<()>>,
    installation: Option<Installation>,
}
impl Session {
    pub fn start(json: bool) -> Self {
        let renderer = Arc::new(Renderer {
            state: Mutex::new(State::new()),
            output: Mutex::new(Output {
                sink: Box::new(std::io::stderr()),
                visible_lines: 0,
                failed: false,
            }),
            wake: Condvar::new(),
            tty: !json && std::io::stderr().is_terminal(),
        });
        let installation = wsp_core::progress::install(renderer.clone());
        let active = renderer.clone();
        let worker = thread::spawn(move || {
            loop {
                let state = active.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.stopped {
                    break;
                }
                let (state, _) = active
                    .wake
                    .wait_timeout(state, TICK)
                    .unwrap_or_else(|e| e.into_inner());
                if state.stopped {
                    break;
                }
                drop(state);
                active.tick();
            }
        });
        Self {
            renderer,
            worker: Some(worker),
            installation: Some(installation),
        }
    }

    pub fn finish(&mut self) {
        self.renderer.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.installation.take();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.finish();
    }
}

pub fn restore_cursor() {
    let _ = std::io::stderr().write_all(b"\x1b[?25h");
}

struct Measurement {
    resource: String,
    phase: String,
    detail: String,
    fraction: Option<Fraction>,
}

#[derive(Clone)]
struct WorkerRow {
    owner: thread::ThreadId,
    label: String,
    complete: bool,
}

fn repository_label(line: &str) -> String {
    let label = line
        .strip_prefix("Fetching ")
        .or_else(|| line.strip_prefix("Validating "))
        .unwrap_or(line)
        .strip_suffix(" for refresh")
        .unwrap_or_else(|| {
            line.strip_prefix("Fetching ")
                .or_else(|| line.strip_prefix("Validating "))
                .unwrap_or(line)
        });
    display_line(label, usize::MAX)
}

struct WorkerFrame<'a> {
    workers: &'a [WorkerRow],
    operations: &'a BTreeMap<u64, String>,
    owners: &'a BTreeMap<u64, thread::ThreadId>,
    measurements: &'a BTreeMap<u64, Measurement>,
}

fn worker_frame(
    state: WorkerFrame<'_>,
    frame: usize,
    animate: bool,
    width: usize,
    height: usize,
) -> Vec<String> {
    let max_rows = height.saturating_sub(2).clamp(2, MAX_WORKER_ROWS);
    let mut ordered = Vec::new();
    let mut active = Vec::new();
    let mut complete = Vec::new();
    for worker in state.workers {
        let operation = state
            .operations
            .iter()
            .rev()
            .find(|(id, _)| state.owners.get(id) == Some(&worker.owner));
        if let Some((id, line)) = operation {
            let operation = Some((*id, line.as_str()));
            ordered.push((worker, operation));
            active.push((worker, operation));
        } else if worker.complete {
            ordered.push((worker, None));
            complete.push(worker);
        }
    }

    let overflow = active.len() + complete.len() > max_rows;
    let visible = if overflow {
        let row_limit = max_rows.saturating_sub(1);
        let active_rows = active.iter().take(row_limit).copied();
        let shown_active = active_rows.len();
        active_rows
            .chain(
                complete
                    .iter()
                    .rev()
                    .take(row_limit - shown_active)
                    .map(|worker| (*worker, None)),
            )
            .collect::<Vec<_>>()
    } else {
        ordered
    };

    let hidden_active = active.len().saturating_sub(visible.len().min(active.len()));
    let shown_complete = visible
        .len()
        .saturating_sub(active.len().min(visible.len()));
    let hidden_complete = complete.len().saturating_sub(shown_complete);
    let bar_width = ROW_BAR_WIDTH + 2;
    let label_width = width.saturating_sub(bar_width + 1 + 3 + 12).min(30);
    let mut lines = Vec::new();
    for (worker, operation) in visible {
        let (bar, status, operation_label) = if let Some((id, line)) = operation {
            if let Some(measurement) = state.measurements.get(&id) {
                let bar = row_progress_bar(
                    measurement.fraction,
                    if animate { frame } else { ROW_BAR_WIDTH },
                )
                .unwrap_or_else(|| format!("[{}]", "░".repeat(ROW_BAR_WIDTH)));
                let status = measured_status(
                    measurement,
                    width.saturating_sub(columns(&bar) + 1 + label_width + 3 + 20),
                );
                (bar, status, operation_name(line))
            } else {
                let bar = row_progress_bar(None, if animate { frame } else { ROW_BAR_WIDTH })
                    .unwrap_or_else(|| format!("[{}]", "░".repeat(ROW_BAR_WIDTH)));
                let status = operation_status(line);
                (bar, status, String::new())
            }
        } else {
            (
                row_progress_bar(
                    Some(Fraction {
                        completed: 1,
                        total: 1,
                    }),
                    frame,
                )
                .unwrap_or_else(|| format!("[{}]", "█".repeat(ROW_BAR_WIDTH))),
                "Done".to_owned(),
                String::new(),
            )
        };
        let label = shorten_label(&worker.label, label_width);
        let label = pad_columns(&label, label_width);
        let remaining = width.saturating_sub(columns(&bar) + 1 + label_width + 3);
        let measured = operation.is_some_and(|(id, _)| state.measurements.contains_key(&id));
        let minimum_status_width = if measured { 4 } else { 1 };
        let operation_budget = remaining
            .saturating_sub(minimum_status_width + 3)
            .saturating_sub(3)
            .min(20);
        let operation_label = if operation_label.is_empty() || operation_budget == 0 {
            String::new()
        } else {
            let operation_label = display_line(&operation_label, operation_budget);
            format!(" · {operation_label}")
        };
        let status_width = remaining
            .saturating_sub(columns(&operation_label))
            .saturating_sub(if operation_label.is_empty() { 0 } else { 3 });
        let status = if measured {
            measured_status(
                state
                    .measurements
                    .get(&operation.expect("measured operation").0)
                    .expect("measurement checked above"),
                status_width,
            )
        } else {
            display_line(&status, status_width)
        };
        lines.push(format!("{bar} {label}{operation_label}   {status}"));
    }
    if hidden_active + hidden_complete > 0 {
        let hidden = hidden_active + hidden_complete;
        let label = format!("… +{hidden} more repositories");
        let label = display_line(&label, label_width);
        let label = pad_columns(&label, label_width);
        lines.push(format!("{} {label}   ", " ".repeat(ROW_BAR_WIDTH + 2)));
    }
    lines
}

fn measured_status(measurement: &Measurement, width: usize) -> String {
    let suffix = measurement
        .fraction
        .filter(|fraction| fraction.total > 0)
        .map(|fraction| {
            let percent = u128::from(fraction.completed.min(fraction.total)) * 100
                / u128::from(fraction.total);
            format!(" {percent}%")
        })
        .unwrap_or_default();
    if width == 0 {
        return String::new();
    }
    let suffix = if columns(&suffix) <= width {
        suffix
    } else {
        display_line(suffix.trim(), width)
    };
    let prefix_width = width.saturating_sub(columns(&suffix));
    let prefix = display_line(
        &format!("{} {}", measurement.phase, measurement.detail),
        prefix_width,
    );
    format!("{prefix}{suffix}")
}

fn operation_status(line: &str) -> String {
    if line.starts_with("Fetching ") {
        return "Fetching".into();
    }
    if line.starts_with("Validating ") {
        return "Validating".into();
    }
    line.rsplit(" · ").next().unwrap_or(line).to_owned()
}

fn operation_name(line: &str) -> String {
    line.split(" · ")
        .nth(1)
        .unwrap_or_default()
        .split(" · ")
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn pad_columns(text: &str, width: usize) -> String {
    let used = columns(text);
    if used >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - used))
    }
}

/// Reserve authoritative measured detail and elapsed before resource names.
#[allow(clippy::too_many_arguments)]
fn frame_line(
    primary: &str,
    measurement: Option<&Measurement>,
    context: Option<&str>,
    context_measurement: Option<&Measurement>,
    additional: usize,
    elapsed: u64,
    bar: Option<&str>,
    width: usize,
) -> String {
    let primary = measurement.map_or_else(
        || display_line(primary, usize::MAX),
        |value| {
            display_line(
                &format!("{} · {} {}", value.resource, value.phase, value.detail),
                usize::MAX,
            )
        },
    );
    let context = context.map(|line| display_line(line, usize::MAX));
    let activity = if additional > 0 {
        format!(" (+{additional} active)")
    } else {
        String::new()
    };
    let elapsed = format!("({elapsed}s)");
    let prefix = bar.map_or(String::new(), |bar| format!("{bar} "));
    let summary = context
        .as_ref()
        .map_or(String::new(), |line| format!(" · {line}"));
    let full = format!("{prefix}{primary}{summary}{activity} {elapsed}");
    if columns(&full) <= width {
        return full;
    }

    let elapsed_width = columns(&elapsed);
    if elapsed_width >= width {
        return display_line(&elapsed, width);
    }
    let summary_count = context_measurement.map_or(String::new(), |value| {
        format!(" · {}", display_line(&value.detail, usize::MAX))
    });
    let (resource, phase, detail) = match measurement {
        Some(value) => (
            display_line(&value.resource, usize::MAX),
            display_line(&value.phase, usize::MAX),
            display_line(&value.detail, usize::MAX),
        ),
        None => (primary, String::new(), String::new()),
    };
    // Extremely narrow terminals retain the leading measured values and elapsed.
    let tail = format!("{detail}{summary_count}{activity}");
    let tail = display_line(&tail, width.saturating_sub(elapsed_width + 2));
    let suffix = if tail.is_empty() {
        format!(" {elapsed}")
    } else {
        format!(" {tail} {elapsed}")
    };
    let available = width.saturating_sub(columns(&suffix));
    // A bar is either complete or absent. Preserve measurements and elapsed
    // before dropping the bar on terminals too narrow to fit both.
    let prefix = if columns(&prefix) + 3 <= available {
        prefix
    } else {
        String::new()
    };
    let available = available.saturating_sub(columns(&prefix));
    // Keep the measured phase intact whenever it fits after reserved numbers.
    let phase = if phase.is_empty() {
        String::new()
    } else {
        format!(" · {phase}")
    };
    let label = if columns(&phase) < available {
        format!(
            "{}{phase}",
            shorten_label(&resource, available - columns(&phase))
        )
    } else {
        shorten_label(&format!("{resource}{phase}"), available)
    };
    format!("{prefix}{label}{suffix}")
}

/// Render authoritative work units without guessing percentages from labels.
fn progress_bar(fraction: Option<Fraction>, frame: usize, animate: bool) -> Option<String> {
    progress_bar_width(fraction, frame, animate, BAR_WIDTH)
}

fn row_progress_bar(fraction: Option<Fraction>, frame: usize) -> Option<String> {
    progress_bar_width(fraction, frame, true, ROW_BAR_WIDTH)
}

fn progress_bar_width(
    fraction: Option<Fraction>,
    frame: usize,
    animate: bool,
    width: usize,
) -> Option<String> {
    let fraction = fraction.filter(|value| value.total > 0);
    let cells = if let Some(value) = fraction {
        let filled = (u128::from(value.completed.min(value.total)) * width as u128
            / u128::from(value.total)) as usize;
        format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
    } else if animate {
        let travel = width - CURSOR_WIDTH;
        let position = frame % (travel * 2);
        let position = if position <= travel {
            position
        } else {
            travel * 2 - position
        };
        format!(
            "{}{}{}",
            "░".repeat(position),
            "█".repeat(CURSOR_WIDTH),
            "░".repeat(travel - position)
        )
    } else {
        return None;
    };
    Some(format!("[{cells}]"))
}

fn columns(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

fn shorten_label(label: &str, width: usize) -> String {
    if columns(label) <= width {
        return label.to_string();
    }
    if width == 0 {
        return String::new();
    }
    // Keep both the resource prefix and the current phase when shortening a
    // long repository identity. Wide Unicode characters consume real columns.
    let tail_width = (width - 1) / 2;
    let head = display_line(label, width - 1 - tail_width);
    let mut used = 0;
    let tail: String = label
        .chars()
        .rev()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= tail_width
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

fn display_line(text: &str, width: usize) -> String {
    let mut safe = String::new();
    for token in text.split_whitespace() {
        if !safe.is_empty() {
            safe.push(' ');
        }
        let clean: String = token
            .chars()
            .filter(|c| !c.is_control() && *c != '\u{7f}')
            .collect();
        if let Some(scheme) = clean.find("://") {
            let authority = scheme + 3;
            let end = clean[authority..]
                .find('/')
                .map_or(clean.len(), |i| authority + i);
            if let Some(at) = clean[authority..end].rfind('@') {
                safe.push_str(&clean[..authority]);
                safe.push_str("[redacted]@");
                safe.push_str(&clean[authority + at + 1..]);
                continue;
            }
        }
        safe.push_str(&clean);
    }
    let mut used = 0;
    safe.chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_bar_uses_authoritative_units_and_bounces_at_both_ends() {
        for (completed, total, expected) in [
            (0, 100, "[░░░░░░░░░░░░]"),
            (25, 100, "[███░░░░░░░░░]"),
            (100, 100, "[████████████]"),
            (200, 100, "[████████████]"),
            (u64::MAX, u64::MAX, "[████████████]"),
        ] {
            for animate in [false, true] {
                assert_eq!(
                    progress_bar(Some(Fraction { completed, total }), 7, animate).as_deref(),
                    Some(expected),
                    "wrong bar for {completed}/{total}"
                );
            }
        }
        let positions = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0,
        ];
        for (frame, position) in positions.into_iter().enumerate() {
            let expected = format!("[{}██{}]", "░".repeat(position), "░".repeat(10 - position));
            for fraction in [
                None,
                Some(Fraction {
                    completed: 9,
                    total: 0,
                }),
            ] {
                assert_eq!(
                    progress_bar(fraction, frame, true).as_deref(),
                    Some(expected.as_str()),
                    "cursor endpoint/cycle mismatch at frame {frame}"
                );
                assert_eq!(
                    progress_bar(fraction, frame, false),
                    None,
                    "unknown work must not animate in append-only output"
                );
            }
        }
    }

    #[test]
    fn measured_phase_transitions_keep_bar_position_and_ignore_parent_fraction() {
        let now = Instant::now();
        for tty in [false, true] {
            let mut state = State::new();
            start(&mut state, 1, "Fetching", now);
            for (index, (fraction, expected)) in [
                (None, "[██░░░░░░░░░░]"),
                (
                    Some(Fraction {
                        completed: 50,
                        total: 100,
                    }),
                    "[██████░░░░░░]",
                ),
                (None, "[░░██░░░░░░░░]"),
            ]
            .into_iter()
            .enumerate()
            {
                state.apply(
                    Event::Measured {
                        id: 1,
                        line: "Fetching".into(),
                        resource: "widgets".into(),
                        phase: "Fetching".into(),
                        detail: "native units".into(),
                        fraction,
                    },
                    now,
                );
                let frame = state
                    .frame(now + Duration::from_secs(index as u64 + 1), tty, 80)
                    .unwrap_or_else(|| panic!("fraction change must produce phase {index} update even when labels are identical"));
                if tty || fraction.is_some() {
                    assert!(
                        frame.text.starts_with(expected),
                        "phase {index} moved or lost bar: {}",
                        frame.text
                    );
                } else {
                    assert!(
                        frame.text.starts_with("widgets"),
                        "plain unknown work must omit bar: {}",
                        frame.text
                    );
                }
            }
        }
        let parent = Measurement {
            resource: "repos".into(),
            phase: "Fetching".into(),
            detail: "1/3".into(),
            fraction: Some(Fraction {
                completed: 1,
                total: 3,
            }),
        };
        let state = Arc::new(Mutex::new(State::new()));
        start(&mut state.lock().unwrap(), 1, "Fetching repos 1/3", now);
        state.lock().unwrap().apply(
            Event::Measured {
                id: 1,
                line: "Fetching repos 1/3".into(),
                resource: parent.resource,
                phase: parent.phase,
                detail: parent.detail,
                fraction: parent.fraction,
            },
            now,
        );
        std::thread::scope(|scope| {
            let worker = state.clone();
            scope
                .spawn(move || start(&mut worker.lock().unwrap(), 2, "Connecting", now))
                .join()
                .unwrap();
        });
        let frame = state
            .lock()
            .unwrap()
            .frame(now + Duration::from_secs(1), true, 80)
            .unwrap();
        assert!(
            frame.text.starts_with("[██░░░░░░░░░░] Connecting"),
            "parent count must not invent percentage for active phase: {}",
            frame.text
        );
        assert!(
            frame.text.contains("1/3"),
            "batch count lost beside active phase: {}",
            frame.text
        );
    }

    #[test]
    fn narrow_frames_shorten_unicode_resource_before_dropping_whole_bar() {
        let measurement = Measurement {
            resource: "界very-long-repository-name".repeat(4),
            phase: "Receiving".into(),
            detail: "42% (42/100)".into(),
            fraction: Some(Fraction {
                completed: 42,
                total: 100,
            }),
        };
        let bar = progress_bar(measurement.fraction, 0, true).unwrap();
        for width in [0, 1, 5, 10, 20, 30, 45, 60, 80, 200] {
            let text = frame_line(
                "ignored presentation",
                Some(&measurement),
                None,
                None,
                0,
                12,
                Some(&bar),
                width,
            );
            assert!(columns(&text) <= width, "width {width} exceeded: {text}");
            if width >= 45 {
                assert!(
                    text.starts_with(&bar),
                    "bar dropped before shortening resource at width {width}: {text}"
                );
                assert!(
                    text.contains("42% (42/100)"),
                    "native units dropped at width {width}: {text}"
                );
                assert!(
                    text.ends_with("(12s)"),
                    "elapsed dropped at width {width}: {text}"
                );
            } else {
                assert!(
                    !text.contains('['),
                    "partial bar leaked at width {width}: {text}"
                );
            }
        }
    }

    #[test]
    fn narrow_frames_preserve_measurements_batch_count_and_elapsed() {
        let now = Instant::now();
        let identity = format!("github.com/{}/widgets", "界very-long-owner".repeat(8));
        let progress = format!(
            "{identity} · Git fetch · Receiving objects [████░░░░] 42% (42/100), 1.00 MiB | 2.00 MiB/s"
        );
        for (offset, tty) in [(12, true), (13, false)] {
            let mut state = State::new();
            start(&mut state, 1, &progress, now);
            state.apply(
                Event::Measured {
                    id: 1,
                    line: progress.clone(),
                    resource: identity.clone(),
                    phase: "Receiving objects".into(),
                    detail: "42% (42/100), 1.00 MiB | 2.00 MiB/s".into(),
                    fraction: Some(Fraction {
                        completed: 42,
                        total: 100,
                    }),
                },
                now,
            );
            let frame = state
                .frame(now + Duration::from_secs(offset), tty, 80)
                .unwrap();
            assert!(
                columns(&frame.text) <= 80,
                "frame exceeds terminal width: {}",
                frame.text
            );
            assert!(
                frame.text.contains("42% (42/100), 1.00 MiB | 2.00 MiB/s"),
                "measured Git detail was hidden: {}",
                frame.text
            );
            assert!(
                frame.text.ends_with(&format!("({offset}s)")),
                "elapsed time was hidden: {}",
                frame.text
            );
            assert!(
                frame.text.contains('…'),
                "long identity must visibly shorten: {}",
                frame.text
            );
        }
        let batch = "Fetching repos 1/3: another-long-repository";
        let measurement = Measurement {
            resource: identity,
            phase: "Receiving objects".into(),
            detail: "42% (42/100), 1.00 MiB | 2.00 MiB/s".into(),
            fraction: Some(Fraction {
                completed: 42,
                total: 100,
            }),
        };
        let batch_count = Measurement {
            resource: "another-long-repository".into(),
            phase: "Fetching repos".into(),
            detail: "1/3".into(),
            fraction: Some(Fraction {
                completed: 1,
                total: 3,
            }),
        };
        let opaque = format!("{} <unstructured renderer style>", measurement.resource);
        let opaque_frame = frame_line(&opaque, Some(&measurement), None, None, 0, 12, None, 80);
        assert!(
            opaque_frame.contains(&measurement.detail),
            "authoritative measurements must not depend on parsing formatted labels: {opaque_frame}"
        );
        let frame = frame_line(
            &progress,
            Some(&measurement),
            Some(batch),
            Some(&batch_count),
            1,
            12,
            Some("[██░░░░░░░░░░]"),
            80,
        );
        assert!(
            frame.contains("42% (42/100), 1.00 MiB | 2.00 MiB/s"),
            "batch displaced measured detail: {frame}"
        );
        assert!(frame.contains("1/3"), "batch count was hidden: {frame}");
        assert!(
            frame.ends_with("(12s)"),
            "batch displaced elapsed time: {frame}"
        );
        assert!(columns(&frame) <= 80);
        for width in [0, 1, 5, 10, 20, 40] {
            let narrow = frame_line(
                &progress,
                Some(&measurement),
                None,
                None,
                0,
                12,
                Some("[██░░░░░░░░░░]"),
                width,
            );
            assert!(
                columns(&narrow) <= width,
                "narrow frame exceeds {width}: {narrow}"
            );
            if width >= 5 {
                assert!(
                    narrow.ends_with("(12s)"),
                    "narrow frame hid elapsed: {narrow}"
                );
            }
        }
    }

    #[test]
    fn worker_rows_align_names_and_retain_completed_repositories() {
        let now = Instant::now();
        let state = Arc::new(Mutex::new(State::new()));
        start(&mut state.lock().unwrap(), 1, "Fetching repos 0/2", now);
        std::thread::scope(|scope| {
            let first = state.clone();
            scope
                .spawn(move || {
                    let mut state = first.lock().unwrap();
                    start(&mut state, 10, "Fetching github.com/demo/alpha", now);
                    start(&mut state, 11, "alpha · Git fetch", now);
                })
                .join()
                .unwrap();
            let second = state.clone();
            scope
                .spawn(move || {
                    let mut state = second.lock().unwrap();
                    start(&mut state, 20, "Fetching github.com/demo/bravo", now);
                    start(&mut state, 21, "bravo · Git fetch", now);
                })
                .join()
                .unwrap();
        });
        let mut state = state.lock().unwrap();
        let frame = state
            .frame(now + Duration::from_secs(1), true, 200)
            .unwrap();
        assert!(frame.in_place, "wsp-owned frames repaint in place");
        let lines: Vec<_> = frame.text.lines().collect();
        assert_eq!(lines.len(), 2, "each repository gets one compact row");
        assert!(lines.iter().all(|line| line.starts_with('[')));
        assert!(lines[0].contains("github.com/demo/alpha"), "{}", frame.text);
        assert!(lines[1].contains("github.com/demo/bravo"), "{}", frame.text);
        assert!(lines.iter().all(|line| line.contains("Git fetch")));
        assert!(
            lines
                .iter()
                .all(|line| line.matches("Git fetch").count() == 1),
            "quiet rows must show the operation once: {lines:?}"
        );
        assert!(lines.iter().all(|line| columns(line) <= 80));

        state.apply(
            Event::Measured {
                id: 21,
                line: "bravo · Git fetch · Receiving objects".into(),
                resource: "bravo".into(),
                phase: "Receiving objects".into(),
                detail: "completed 42/100, with a deliberately long throughput detail that must be clipped to fit the terminal width".into(),
                fraction: Some(Fraction {
                    completed: 42,
                    total: 100,
                }),
            },
            now + Duration::from_secs(1),
        );
        let frame = state
            .frame_with_height(now + Duration::from_secs(2), true, 80, 24)
            .unwrap();
        assert!(
            frame.text.lines().all(|line| columns(line) <= 80),
            "measured rows must not wrap: {}",
            frame.text
        );
        assert!(frame.text.contains("42%"));

        state.apply(
            Event::Measured {
                id: 21,
                line: "bravo · Git for-each-ref with a deliberately long nested operation label · Receiving objects".into(),
                resource: "bravo".into(),
                phase: "Receiving objects in a deliberately long phase".into(),
                detail: "completed 42/100 with a long transfer rate".into(),
                fraction: Some(Fraction {
                    completed: 42,
                    total: 100,
                }),
            },
            now + Duration::from_secs(2),
        );
        let narrow = state
            .frame_with_height(now + Duration::from_secs(3), true, 31, 4)
            .unwrap();
        assert!(
            narrow.text.lines().all(|line| columns(line) <= 31),
            "measured nested operations must fit narrow terminals: {}",
            narrow.text
        );
        assert!(narrow.text.contains("42%"), "{}", narrow.text);

        state.apply(
            Event::Measured {
                id: 21,
                line: "bravo · Git for-each-ref with a deliberately long nested operation label · Receiving objects".into(),
                resource: "bravo".into(),
                phase: "Receiving objects".into(),
                detail: "completed 100/100".into(),
                fraction: Some(Fraction {
                    completed: 100,
                    total: 100,
                }),
            },
            now + Duration::from_secs(3),
        );
        let complete = state
            .frame_with_height(now + Duration::from_secs(4), true, 31, 4)
            .unwrap();
        assert!(
            complete.text.lines().all(|line| columns(line) <= 31),
            "100% measured rows must fit narrow terminals: {}",
            complete.text
        );
        assert!(complete.text.contains("100%"), "{}", complete.text);

        state.apply(Event::Finished { id: 11 }, now + Duration::from_secs(5));
        state.apply(Event::Finished { id: 10 }, now + Duration::from_secs(5));
        let frame = state
            .frame(now + Duration::from_secs(6), true, 200)
            .unwrap();
        assert!(
            frame.text.contains("github.com/demo/alpha"),
            "{}",
            frame.text
        );
        assert!(frame.text.contains("Done"));
        assert!(frame.text.contains("github.com/demo/bravo"));
        assert!(frame.text.contains("Git for-each-ref"));

        state.apply(Event::Finished { id: 21 }, now + Duration::from_secs(7));
        state.apply(Event::Finished { id: 20 }, now + Duration::from_secs(7));
        let frame = state.frame(now + Duration::from_secs(8), true, 80).unwrap();
        assert!(frame.text.contains("Fetching repos 0/2"));
        assert!(!frame.text.contains("Done"));
    }

    #[test]
    fn multirow_layout_falls_back_when_terminal_cannot_fit_the_panel() {
        let now = Instant::now();
        let state = Arc::new(Mutex::new(State::new()));
        start(&mut state.lock().unwrap(), 1, "Fetching repos 0/2", now);
        std::thread::scope(|scope| {
            for (id, identity) in [
                (10, "Fetching github.com/demo/alpha"),
                (20, "Fetching github.com/demo/bravo"),
            ] {
                let state = state.clone();
                scope
                    .spawn(move || start(&mut state.lock().unwrap(), id, identity, now))
                    .join()
                    .unwrap();
            }
        });
        let mut state = state.lock().unwrap();
        for (width, height) in [(12, 24), (26, 24), (80, 3)] {
            let frame = state
                .frame_with_height(now + Duration::from_secs(1), true, width, height)
                .unwrap();
            assert_eq!(frame.text.lines().count(), 1);
            assert!(
                columns(&frame.text) <= width,
                "{width} columns: {}",
                frame.text
            );
        }
        let frame = state
            .frame_with_height(now + Duration::from_secs(4), true, 31, 4)
            .unwrap();
        assert_eq!(frame.text.lines().count(), 2);
        assert!(frame.text.lines().all(|line| columns(line) <= 31));
    }

    struct RecordingSink(Arc<Mutex<Vec<u8>>>);
    impl Write for RecordingSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn recording_renderer() -> (Renderer, Arc<Mutex<Vec<u8>>>) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        (
            Renderer {
                state: Mutex::new(State::new()),
                output: Mutex::new(Output {
                    sink: Box::new(RecordingSink(bytes.clone())),
                    visible_lines: 0,
                    failed: false,
                }),
                wake: Condvar::new(),
                tty: true,
            },
            bytes,
        )
    }

    #[test]
    fn permanent_output_clears_the_line_and_shutdown_preserves_diagnostics() {
        let (renderer, bytes) = recording_renderer();
        let now = Instant::now();
        start(&mut renderer.state.lock().unwrap(), 1, "Waiting", now);
        let frame = renderer
            .state
            .lock()
            .unwrap()
            .frame(now + Duration::from_secs(1), true, 80)
            .unwrap();
        renderer.publish(frame);
        renderer.observe(Event::Message("warning: permanent".into()));
        renderer.observe(Event::Diagnostic("remote: important".into()));
        renderer.stop();
        let written = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(written.contains("\r\x1b[2Kwarning: permanent\n"));
        assert!(written.ends_with("remote: important\n"));
        assert_eq!(renderer.output.lock().unwrap().visible_lines, 0);
    }

    #[test]
    fn terminal_context_sanitizes_repository_controls() {
        for label in ["alpha\x1b[2J", "alpha\nforged", "alpha\u{009b}2J"] {
            let (renderer, bytes) = recording_renderer();
            renderer.observe(Event::TerminalContext(format!("Fetching {label}")));
            let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
            assert_eq!(
                output,
                format!("{}\n", display_line(&format!("Fetching {label}"), 4096))
            );
            assert!(!output.contains('\x1b') && !output.contains('\u{009b}'));
            assert_eq!(output.lines().count(), 1);
        }
    }

    #[test]
    fn terminal_output_requires_a_human_interactive_session() {
        for tty in [false, true] {
            let (mut renderer, _) = recording_renderer();
            renderer.tty = tty;
            assert_eq!(renderer.terminal_output(), tty);
        }
    }

    #[test]
    fn terminal_yield_defers_every_writer_until_the_last_resume() {
        for nested in [
            Event::Yielded("bravo · Git fetch".into()),
            Event::Suspended(true),
        ] {
            let (renderer, bytes) = recording_renderer();
            let now = Instant::now();
            start(
                &mut renderer.state.lock().unwrap(),
                1,
                "Fetching repos",
                now,
            );
            let prepared = renderer
                .state
                .lock()
                .unwrap()
                .frame(now + Duration::from_secs(1), true, 80)
                .unwrap();
            assert!(renderer.publish(Frame {
                generation: prepared.generation,
                text: prepared.text.clone(),
                in_place: true,
            }));
            renderer.observe(Event::Yielded("alpha · Git fetch".into()));
            assert_eq!(renderer.output.lock().unwrap().visible_lines, 0);
            let handed_off = bytes.lock().unwrap().clone();
            assert!(
                String::from_utf8(handed_off.clone())
                    .unwrap()
                    .ends_with("\r\x1b[2Kalpha · Git fetch\n")
            );
            renderer.observe(Event::Diagnostic("remote: deferred".into()));
            renderer.observe(nested.clone());
            renderer.observe(Event::Message("warning: deferred\nsecond line".into()));
            renderer.observe(Event::External(true));
            renderer.observe(Event::External(false));
            renderer.tick();
            renderer.publish(prepared);
            renderer.observe(Event::Suspended(false));
            renderer.tick();
            assert_eq!(
                *bytes.lock().unwrap(),
                handed_off,
                "no writer may touch a child's terminal until the final resume"
            );
            renderer.observe(Event::Suspended(false));
            let resumed = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
            assert!(resumed.contains("remote: deferred\n"));
            assert!(resumed.ends_with("warning: deferred\nsecond line\n"));
            if let Event::Yielded(label) = nested {
                assert!(resumed.contains(&format!("{label}\n")));
            }
            assert_eq!(renderer.state.lock().unwrap().suspended, 0);
            let resumed_frame = renderer
                .state
                .lock()
                .unwrap()
                .frame(now + Duration::from_secs(2), true, 80)
                .expect("animation resumes after the final child returns");
            assert!(renderer.publish(resumed_frame));
            assert_eq!(renderer.output.lock().unwrap().visible_lines, 1);
        }
    }

    #[test]
    fn terminal_yield_shutdown_waits_for_outstanding_children() {
        let (renderer, bytes) = recording_renderer();
        renderer.observe(Event::Yielded("alpha · Git fetch".into()));
        renderer.observe(Event::Yielded("bravo · Git fetch".into()));
        renderer.observe(Event::Message("operation cancelled".into()));
        renderer.observe(Event::Diagnostic("remote: final diagnostic".into()));
        let handed_off = bytes.lock().unwrap().clone();
        renderer.stop();
        renderer.observe(Event::Suspended(false));
        renderer.tick();
        assert_eq!(*bytes.lock().unwrap(), handed_off);
        renderer.observe(Event::Suspended(false));
        let written = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(written.contains("remote: final diagnostic\n"));
        assert!(written.contains("bravo · Git fetch\n"));
        assert!(written.ends_with("operation cancelled\nremote: final diagnostic\n"));
        assert_eq!(renderer.output.lock().unwrap().visible_lines, 0);
    }

    #[test]
    fn output_clears_a_multiline_frame_without_leaving_stale_rows() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut output = Output {
            sink: Box::new(RecordingSink(bytes.clone())),
            visible_lines: 3,
            failed: false,
        };
        output.clear().unwrap();
        assert_eq!(
            String::from_utf8(bytes.lock().unwrap().clone()).unwrap(),
            "\x1b[?25h\r\x1b[2A\x1b[J"
        );
        assert_eq!(output.visible_lines, 0);
    }

    #[test]
    fn cursor_visibility_follows_terminal_ownership() {
        for (in_place, handoff) in [(true, false), (true, true), (false, false)] {
            let (renderer, bytes) = recording_renderer();
            let generation = renderer.state.lock().unwrap().generation;
            renderer.publish(Frame {
                generation,
                text: "[   ██   ] alpha Fetching".into(),
                in_place,
            });
            let frame = bytes.lock().unwrap().clone();
            assert_eq!(frame.windows(6).any(|w| w == b"\x1b[?25l"), in_place);
            if handoff {
                renderer.observe(Event::Yielded("alpha · Git fetch".into()));
                let written = bytes.lock().unwrap().clone();
                assert!(written.windows(6).any(|w| w == b"\x1b[?25h"));
                assert_eq!(renderer.output.lock().unwrap().visible_lines, 0);
                renderer.observe(Event::Suspended(false));
            }
            renderer.stop();
            let written = bytes.lock().unwrap().clone();
            assert_eq!(written.windows(6).any(|w| w == b"\x1b[?25h"), in_place);
            assert_eq!(renderer.output.lock().unwrap().visible_lines, 0);
        }
    }

    #[test]
    fn finished_worker_events_do_not_write_to_the_sink() {
        let (renderer, bytes) = recording_renderer();
        let now = Instant::now();
        start(&mut renderer.state.lock().unwrap(), 1, "Waiting", now);
        let frame = renderer
            .state
            .lock()
            .unwrap()
            .frame(now + Duration::from_secs(1), true, 80)
            .unwrap();
        renderer.publish(frame);
        let before = bytes.lock().unwrap().clone();
        renderer.observe(Event::Finished { id: 1 });
        assert_eq!(*bytes.lock().unwrap(), before);
        renderer.stop();
        assert!(bytes.lock().unwrap().len() > before.len());
    }

    #[test]
    fn closed_sink_disables_optional_messages_without_panicking() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (renderer, _) = recording_renderer();
        renderer.output.lock().unwrap().sink = Box::new(Closed);
        assert!(!renderer.observe(Event::Message("warning".into())));
        assert!(!renderer.observe(Event::Message("later".into())));
        renderer.stop();
    }
    fn start(state: &mut State, id: u64, line: &str, now: Instant) {
        state.apply(
            Event::Started {
                id,
                line: line.into(),
            },
            now,
        );
    }
    #[test]
    fn reveal_accumulates_fast_phases_but_excludes_user_wait() {
        let now = Instant::now();
        let mut state = State::new();
        start(&mut state, 1, "first", now);
        assert!(
            state
                .frame(now + Duration::from_millis(300), true, 80)
                .is_none()
        );
        state.apply(Event::Finished { id: 1 }, now + Duration::from_millis(300));
        start(&mut state, 2, "second", now + Duration::from_millis(300));
        state.apply(Event::Suspended(true), now + Duration::from_millis(400));
        state.apply(Event::Suspended(false), now + Duration::from_secs(10));
        assert!(
            state
                .frame(now + Duration::from_millis(10050), true, 80)
                .is_none()
        );
        assert!(
            state
                .frame(now + Duration::from_millis(10100), true, 80)
                .unwrap()
                .text
                .contains("second")
        );
    }
    #[test]
    fn plain_output_is_limited_and_has_a_quiet_heartbeat() {
        let now = Instant::now();
        let mut state = State::new();
        start(&mut state, 1, "Waiting for Git", now);
        assert!(
            state
                .frame(now + Duration::from_millis(500), false, 80)
                .is_some()
        );
        state.apply(
            Event::Updated {
                id: 1,
                line: "Receiving objects: 10%".into(),
            },
            now + Duration::from_millis(600),
        );
        assert!(
            state
                .frame(now + Duration::from_millis(1000), false, 80)
                .is_none()
        );
        assert!(
            state
                .frame(now + Duration::from_millis(1500), false, 80)
                .is_some()
        );
        assert!(
            state
                .frame(now + Duration::from_secs(10), false, 80)
                .is_none()
        );
        assert!(
            state
                .frame(now + Duration::from_millis(11500), false, 80)
                .is_some()
        );
    }
    #[test]
    fn sanitizes_credentials_controls_and_wide_characters() {
        assert_eq!(
            display_line("https://alice:secret@example.com/repo\x1b\n界界", 200),
            "https://[redacted]@example.com/repo 界界"
        );
        assert_eq!(display_line("界界界", 5), "界界");
    }
    #[test]
    fn handoff_and_finish_reject_prepared_frames_and_late_events() {
        let renderer = Renderer {
            state: Mutex::new(State::new()),
            output: Mutex::new(Output {
                sink: Box::new(Vec::<u8>::new()),
                visible_lines: 0,
                failed: false,
            }),
            wake: Condvar::new(),
            tty: true,
        };
        let now = Instant::now();
        start(&mut renderer.state.lock().unwrap(), 1, "Working", now);
        let frame = renderer
            .state
            .lock()
            .unwrap()
            .frame(now + Duration::from_secs(1), true, 80)
            .unwrap();
        renderer.observe(Event::Suspended(true));
        renderer.publish(frame);
        assert_eq!(renderer.output.lock().unwrap().visible_lines, 0);
        renderer.observe(Event::Suspended(false));
        let frame = renderer
            .state
            .lock()
            .unwrap()
            .frame(now + Duration::from_secs(2), true, 80)
            .unwrap();
        renderer.stop();
        renderer.publish(frame);
        renderer.observe(Event::Started {
            id: 2,
            line: "late".into(),
        });
        assert_eq!(renderer.output.lock().unwrap().visible_lines, 0);
        assert!(!renderer.state.lock().unwrap().operations.contains_key(&2));
    }
    #[test]
    fn concurrent_terminal_children_keep_all_frames_append_only() {
        let now = Instant::now();
        let mut state = State::new();
        start(&mut state, 1, "Working", now);
        state.apply(Event::External(true), now);
        state.apply(Event::External(true), now);
        state.apply(Event::External(false), now);
        assert!(
            !state
                .frame(now + Duration::from_secs(1), true, 80)
                .unwrap()
                .in_place
        );
        state.apply(Event::External(false), now + Duration::from_secs(1));
        assert!(
            state
                .frame(now + Duration::from_secs(2), true, 80)
                .unwrap()
                .in_place
        );
    }
    #[test]
    fn external_multirow_progress_moves_twice_then_stops_appending() {
        fn external_state(now: Instant, count: usize) -> Arc<Mutex<State>> {
            let state = Arc::new(Mutex::new(State::new()));
            start(&mut state.lock().unwrap(), 1, "Fetching repos", now);
            thread::scope(|scope| {
                for index in 0..count {
                    let state = state.clone();
                    let identity = format!("Fetching github.com/demo/repo-{index}");
                    scope
                        .spawn(move || {
                            start(
                                &mut state.lock().unwrap(),
                                index as u64 + 10,
                                &identity,
                                now,
                            )
                        })
                        .join()
                        .unwrap();
                }
            });
            state.lock().unwrap().apply(Event::External(true), now);
            state
        }

        let now = Instant::now();
        let state = external_state(now, 2);
        let mut state = state.lock().unwrap();

        let first = state
            .frame_with_height(now + Duration::from_secs(1), true, 80, 24)
            .unwrap();
        let second = state
            .frame_with_height(now + Duration::from_secs(2), true, 80, 24)
            .unwrap();
        assert_ne!(first.text, second.text);
        assert!(!first.in_place && !second.in_place);
        assert!(
            state
                .frame_with_height(now + Duration::from_secs(3), true, 80, 24)
                .is_none(),
            "external animation must stop to keep a child prompt visible"
        );
        state.apply(
            Event::Updated {
                id: 20,
                line: "Receiving github.com/demo/bravo".into(),
            },
            now + Duration::from_secs(4),
        );
        assert!(
            state
                .frame_with_height(now + Duration::from_secs(4), true, 80, 24)
                .is_none(),
            "status changes must not reopen the terminal output budget"
        );
        drop(state);

        let compact_terminal = external_state(now, 8);
        let mut compact_terminal = compact_terminal.lock().unwrap();
        let first = compact_terminal
            .frame_with_height(now + Duration::from_secs(1), true, 80, 8)
            .unwrap();
        let second = compact_terminal
            .frame_with_height(now + Duration::from_secs(2), true, 80, 8)
            .unwrap();
        assert!(first.text.lines().count() <= 3, "{}", first.text);
        assert!(second.text.lines().count() <= 3, "{}", second.text);
        assert!(
            compact_terminal.external_lines <= 6,
            "{} lines were appended into an 8-row terminal",
            compact_terminal.external_lines
        );
        drop(compact_terminal);

        let five_rows = external_state(now, 8);
        let mut five_rows = five_rows.lock().unwrap();
        let frame = five_rows
            .frame_with_height(now + Duration::from_secs(1), true, 80, 5)
            .unwrap();
        assert_eq!(frame.text.lines().count(), 2);
        assert!(
            five_rows
                .frame_with_height(now + Duration::from_secs(2), true, 80, 5)
                .is_none()
        );
        assert!(five_rows.external_lines <= 3);
        drop(five_rows);

        let short_terminal = external_state(now, 2);
        let mut short_terminal = short_terminal.lock().unwrap();
        let frame = short_terminal
            .frame_with_height(now + Duration::from_secs(1), true, 80, 4)
            .unwrap();
        assert_eq!(frame.text.lines().count(), 2);
        assert!(
            short_terminal
                .frame_with_height(now + Duration::from_secs(2), true, 80, 4)
                .is_none()
        );

        let narrow_terminal = external_state(now, 2);
        let mut narrow_terminal = narrow_terminal.lock().unwrap();
        assert!(
            narrow_terminal
                .frame_with_height(now + Duration::from_secs(1), true, 12, 24)
                .unwrap()
                .text
                .lines()
                .count()
                == 1
        );
        assert!(
            narrow_terminal
                .frame_with_height(now + Duration::from_secs(2), true, 12, 24)
                .unwrap()
                .text
                .lines()
                .count()
                == 1
        );
        assert!(
            narrow_terminal
                .frame_with_height(now + Duration::from_secs(3), true, 12, 24)
                .is_none()
        );
        drop(narrow_terminal);

        let resized_terminal = external_state(now, 2);
        let mut resized_terminal = resized_terminal.lock().unwrap();
        assert_eq!(
            resized_terminal
                .frame_with_height(now + Duration::from_secs(1), true, 12, 24)
                .unwrap()
                .text
                .lines()
                .count(),
            1
        );
        assert!(
            resized_terminal
                .frame_with_height(now + Duration::from_secs(2), true, 80, 4)
                .is_none()
        );
    }
    #[test]
    fn diagnostic_flood_is_bounded_and_unknown_updates_are_ignored() {
        let mut state = State::new();
        let now = Instant::now();
        for _ in 0..1000 {
            state.apply(Event::Diagnostic("warning".into()), now);
        }
        assert_eq!(state.diagnostics, 64);
        assert!(state.suppressed);
        state.apply(
            Event::Updated {
                id: 99,
                line: "late".into(),
            },
            now,
        );
        assert!(state.operations.is_empty());
    }
}
