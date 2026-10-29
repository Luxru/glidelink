use log::{Level, LevelFilter, Log, Metadata, Record};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const MAX_LINES: usize = 2000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: u64,
    pub elapsed_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

struct Logger {
    level: AtomicU8,
    next_id: AtomicU64,
    started: Instant,
    lines: Mutex<VecDeque<LogEntry>>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn level_code(level: Level) -> u8 {
    match level {
        Level::Error => 1,
        Level::Warn => 2,
        Level::Info => 3,
        Level::Debug => 4,
        Level::Trace => 5,
    }
}

fn filter_code(filter: LevelFilter) -> u8 {
    match filter {
        LevelFilter::Off => 0,
        LevelFilter::Error => 1,
        LevelFilter::Warn => 2,
        LevelFilter::Info => 3,
        LevelFilter::Debug => 4,
        LevelFilter::Trace => 5,
    }
}

pub fn parse_level(value: &str) -> Option<LevelFilter> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" | "warning" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

impl Logger {
    fn new() -> Self {
        Self {
            level: AtomicU8::new(filter_code(LevelFilter::Info)),
            next_id: AtomicU64::new(1),
            started: Instant::now(),
            lines: Mutex::new(VecDeque::with_capacity(MAX_LINES)),
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        level_code(metadata.level()) <= self.level.load(Ordering::Acquire)
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        let level = record.level().as_str().to_string();
        let target = record.target().to_string();
        let message = record.args().to_string();
        eprintln!(
            "[{:>8.3}s] {:<5} {}: {}",
            elapsed_ms as f64 / 1000.0,
            level,
            target,
            message
        );
        if let Ok(mut lines) = self.lines.lock() {
            lines.push_back(LogEntry {
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                elapsed_ms,
                level,
                target,
                message,
            });
            while lines.len() > MAX_LINES {
                lines.pop_front();
            }
        }
    }

    fn flush(&self) {}
}

pub fn init(default_level: &str) {
    let logger = LOGGER.get_or_init(Logger::new);
    let _ = log::set_logger(logger);
    log::set_max_level(LevelFilter::Trace);
    let _ = set_level(default_level);
}

pub fn set_level(value: &str) -> Result<(), String> {
    let filter = parse_level(value).ok_or_else(|| "E_LOG_LEVEL".to_string())?;
    if let Some(logger) = LOGGER.get() {
        logger.level.store(filter_code(filter), Ordering::Release);
    }
    Ok(())
}

pub fn snapshot() -> Vec<LogEntry> {
    LOGGER
        .get()
        .and_then(|logger| {
            logger
                .lines
                .lock()
                .ok()
                .map(|lines| lines.iter().cloned().collect())
        })
        .unwrap_or_default()
}

pub fn clear() {
    if let Some(logger) = LOGGER.get() {
        if let Ok(mut lines) = logger.lines.lock() {
            lines.clear();
        }
    }
}
