//! One output owner for delayed invocation progress and terminal handoffs.
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;
use wsp_core::progress::{Event, Installation, Observer};

const DIAGNOSTICS: usize = 64;
const TICK: Duration = Duration::from_millis(100);

struct State {
    operations: BTreeMap<u64, String>,
    owners: BTreeMap<u64, thread::ThreadId>,
    measurements: BTreeMap<u64, Measurement>,
    invocation_thread: thread::ThreadId,
    diagnostics: VecDeque<String>,
    suppressed: bool,
    generation: u64,
    stopped: bool,
    suspended: usize,
    external: usize,
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
            invocation_thread: thread::current().id(),
            diagnostics: VecDeque::new(),
            suppressed: false,
            generation: 0,
            stopped: false,
            suspended: 0,
            external: 0,
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
                self.operations.insert(id, line);
                self.owners.insert(id, thread::current().id());
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
            } => {
                if let Some(current) = self.operations.get_mut(&id) {
                    *current = line;
                    self.measurements.insert(
                        id,
                        Measurement {
                            resource,
                            phase,
                            detail,
                        },
                    );
                }
            }
            Event::Finished { id } => {
                self.generation += 1;
                self.operations.remove(&id);
                self.owners.remove(&id);
                self.measurements.remove(&id);
            }
            Event::Diagnostic(line) => {
                if self.diagnostics.len() < DIAGNOSTICS {
                    self.diagnostics.push_back(display_line(&line, 4096));
                } else {
                    self.suppressed = true;
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
                    self.external += 1;
                } else {
                    self.external = self.external.saturating_sub(1);
                }
            }
            Event::Message(_) => unreachable!("messages use the output gate"),
        }
        self.resume(now);
    }

    fn frame(&mut self, now: Instant, tty: bool, width: usize) -> Option<Frame> {
        if self.stopped
            || self.suspended > 0
            || self.operations.is_empty()
            || self.work_elapsed(now) < wsp_core::progress::PROGRESS_REVEAL_DELAY
        {
            return None;
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
        let elapsed = self.work_elapsed(now).as_secs();
        let spinner = if in_place {
            let spinner = ['|', '/', '-', '\\'][self.frame % 4];
            self.frame += 1;
            Some(spinner)
        } else {
            None
        };
        Some(Frame {
            generation: self.generation,
            text: frame_line(
                primary,
                self.measurements.get(&primary_id),
                context,
                context_measurement,
                additional,
                elapsed,
                spinner,
                width,
            ),
            in_place,
        })
    }
}

fn take_diagnostics(state: &mut State) -> Vec<String> {
    let mut diagnostics: Vec<_> = state.diagnostics.drain(..).collect();
    if std::mem::take(&mut state.suppressed) {
        diagnostics.push("Additional live diagnostics suppressed".into());
    }
    diagnostics
}

struct Frame {
    generation: u64,
    text: String,
    in_place: bool,
}
struct Output {
    sink: Box<dyn Write + Send>,
    visible: bool,
    failed: bool,
}
impl Output {
    fn clear(&mut self) -> std::io::Result<()> {
        if self.visible {
            self.visible = false;
            self.sink.write_all(b"\r\x1b[2K")?;
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
            output.visible = true;
            write!(output.sink, "\r\x1b[2K{}", frame.text).and_then(|_| output.sink.flush())
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
            let diagnostics = take_diagnostics(&mut state);
            let idle = state.operations.is_empty();
            drop(state);
            if idle {
                output.failed |= output.clear().is_err();
            }
            for line in diagnostics {
                output.failed |= output.message(&display_line(&line, width)).is_err();
            }
        }
        let frame = self.state.lock().unwrap_or_else(|e| e.into_inner()).frame(
            Instant::now(),
            self.tty,
            width,
        );
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
        let diagnostics = take_diagnostics(&mut state);
        drop(state);
        for line in diagnostics {
            let _ = output.message(&display_line(&line, 4096));
        }
        let _ = output.clear();
        self.wake.notify_all();
    }
}

impl Observer for Renderer {
    fn observe(&self, event: Event) -> bool {
        match event {
            Event::Message(line) => {
                let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .generation += 1;
                if output.failed {
                    return false;
                }
                output.failed = output.message(&line).is_err();
                !output.failed
            }
            event @ (Event::Suspended(_) | Event::External(_)) => {
                let mut output = self.output.lock().unwrap_or_else(|e| e.into_inner());
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.stopped {
                    return true;
                }
                state.apply(event, Instant::now());
                let diagnostics = take_diagnostics(&mut state);
                let clear =
                    state.suspended > 0 || state.external > 0 || state.operations.is_empty();
                drop(state);
                for line in diagnostics {
                    output.failed |= output.message(&line).is_err();
                }
                if clear {
                    output.failed |= output.clear().is_err();
                }
                !output.failed
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
                visible: false,
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
    spinner: Option<char>,
    width: usize,
) -> String {
    let primary = display_line(primary, usize::MAX);
    let context = context.map(|line| display_line(line, usize::MAX));
    let activity = if additional > 0 {
        format!(" (+{additional} active)")
    } else {
        String::new()
    };
    let elapsed = format!("({elapsed}s)");
    let prefix = spinner.map_or(String::new(), |spinner| format!("{spinner} "));
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
    let summary_count =
        context_measurement.map_or(String::new(), |value| format!(" · {}", value.detail));
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
    let prefix = display_line(&prefix, available);
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
        };
        let batch_count = Measurement {
            resource: "another-long-repository".into(),
            phase: "Fetching repos".into(),
            detail: "1/3".into(),
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
            Some('|'),
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
                Some('|'),
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
    fn worker_rotation_selects_each_threads_leaf_without_counting_ancestors() {
        let now = Instant::now();
        let state = Arc::new(Mutex::new(State::new()));
        start(&mut state.lock().unwrap(), 1, "Fetching repos 0/2", now);
        std::thread::scope(|scope| {
            let first = state.clone();
            scope
                .spawn(move || {
                    let mut state = first.lock().unwrap();
                    start(&mut state, 10, "First parent", now);
                    start(&mut state, 11, "First leaf", now);
                })
                .join()
                .unwrap();
            let second = state.clone();
            scope
                .spawn(move || {
                    let mut state = second.lock().unwrap();
                    start(&mut state, 20, "Second parent", now);
                    start(&mut state, 21, "Second leaf", now);
                })
                .join()
                .unwrap();
        });
        let mut state = state.lock().unwrap();
        let first = state
            .frame(now + Duration::from_secs(1), true, 200)
            .unwrap();
        let second = state
            .frame(now + Duration::from_secs(3), true, 200)
            .unwrap();
        assert!(
            first.text.contains("Second leaf"),
            "latest worker leaf missing: {}",
            first.text
        );
        assert!(
            second.text.contains("First leaf"),
            "rotation must reach other worker leaf: {}",
            second.text
        );
        for frame in [first, second] {
            assert!(
                !frame.text.contains("parent"),
                "nested ancestor selected: {}",
                frame.text
            );
            assert!(
                frame.text.contains("(+1 active)"),
                "nested scopes inflated worker count: {}",
                frame.text
            );
            assert!(
                frame.text.contains("Fetching repos 0/2"),
                "batch summary missing: {}",
                frame.text
            );
        }
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
                    visible: false,
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
        assert!(!renderer.output.lock().unwrap().visible);
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
                visible: false,
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
        assert!(!renderer.output.lock().unwrap().visible);
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
        assert!(!renderer.output.lock().unwrap().visible);
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
    fn diagnostic_flood_is_bounded_and_unknown_updates_are_ignored() {
        let mut state = State::new();
        let now = Instant::now();
        for _ in 0..1000 {
            state.apply(Event::Diagnostic("warning".into()), now);
        }
        assert_eq!(state.diagnostics.len(), 64);
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
