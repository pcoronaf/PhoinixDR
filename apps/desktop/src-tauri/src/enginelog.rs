//! Live engine log for the Advanced mode.
//!
//! The recovery engine reports what it does through `tracing`, which the
//! command line prints with `-v`. This layer forwards the same records to
//! the window as `engine-log` events (batched, at most a few times per
//! second) so the desktop application can show them while a scan runs. It
//! is switched on and off at runtime and costs a single atomic load per
//! record when off. Records are structure and counts; the engine never logs
//! recovered content.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::field::{Field, Visit};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// One forwarded record.
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    /// Milliseconds since the Unix epoch.
    pub time: u64,
    /// `error`, `warn`, `info`, `debug` or `trace`.
    pub level: &'static str,
    /// The module that emitted the record (`phoinix_fs_fat::volume`).
    pub target: String,
    /// The message followed by the record's fields (`key=value`).
    pub message: String,
}

/// The runtime switch, managed by Tauri.
pub struct EngineLogSwitch {
    enabled: Arc<AtomicBool>,
}

impl EngineLogSwitch {
    /// Turns forwarding on or off.
    pub fn set(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Whether forwarding is on.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

/// The `tracing` layer.
pub struct EngineLogLayer {
    enabled: Arc<AtomicBool>,
    tx: SyncSender<LogLine>,
}

/// Records per batch at most; older records are dropped when the window
/// cannot keep up rather than slowing the engine down.
const QUEUE: usize = 4096;
const BATCH: usize = 256;
const FLUSH: Duration = Duration::from_millis(80);

/// Builds the layer, its switch (off) and the receiving end for
/// [`forward`].
pub fn layer() -> (EngineLogLayer, EngineLogSwitch, Receiver<LogLine>) {
    let enabled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = sync_channel(QUEUE);
    (
        EngineLogLayer {
            enabled: Arc::clone(&enabled),
            tx,
        },
        EngineLogSwitch { enabled },
        rx,
    )
}

/// Starts the thread that batches records and emits them as `engine-log`
/// events. Ends when the layer is dropped.
pub fn forward(app: AppHandle, rx: Receiver<LogLine>) {
    std::thread::Builder::new()
        .name("engine-log".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                let deadline = Instant::now() + FLUSH;
                while batch.len() < BATCH {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(line) => batch.push(line),
                        Err(_) => break,
                    }
                }
                if app.emit("engine-log", &batch).is_err() {
                    break;
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| tracing::warn!(error = %e, "engine log thread not started"));
}

fn wanted(meta: &Metadata<'_>) -> bool {
    *meta.level() <= Level::DEBUG && meta.target().starts_with("phoinix")
}

fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        Level::DEBUG => "debug",
        Level::TRACE => "trace",
    }
}

#[derive(Default)]
struct Fields {
    message: String,
    rest: Vec<String>,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.rest.push(format!("{}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.rest.push(format!("{}={value}", field.name()));
        }
    }
}

impl<S: Subscriber> Layer<S> for EngineLogLayer {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if wanted(meta) {
            Interest::sometimes()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        self.enabled.load(Ordering::Relaxed) && wanted(meta)
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if !self.enabled.load(Ordering::Relaxed) || !wanted(meta) {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut message = fields.message;
        if !fields.rest.is_empty() {
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&fields.rest.join(" "));
        }
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        // Dropping a record when the queue is full is deliberate.
        let _ = self.tx.try_send(LogLine {
            time,
            level: level_name(meta.level()),
            target: meta.target().to_owned(),
            message,
        });
    }
}
