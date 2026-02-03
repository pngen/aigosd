use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::time::SystemTime;

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level { Debug, Info, Warn, Error }

impl Level {
    fn as_str(&self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

struct Logger {
    file: Option<File>,
    structured: bool,
    min_level: Level,
}

impl Logger {
    fn format_msg(&self, level: Level, msg: &str) -> String {
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if self.structured {
            format!(r#"{{"ts":{},"level":"{}","msg":"{}"}}"#, ts, level.as_str(), msg.replace('"', "\\\""))
        } else {
            format!("[{}] [{}] {}", ts, level.as_str(), msg)
        }
    }

    fn write(&mut self, level: Level, msg: &str) {
        if (level as u8) < (self.min_level as u8) { return; }
        let line = self.format_msg(level, msg);
        if let Some(ref mut f) = self.file {
            let _ = writeln!(f, "{}", line);
        }
        eprintln!("{}", line);
    }
}

pub fn init(log_type: &str, file_path: Option<&str>) {
    let file = file_path.and_then(|p| {
        OpenOptions::new().create(true).append(true).open(p).ok()
    });
    let logger = Logger {
        file,
        structured: log_type == "structured",
        min_level: Level::Info,
    };
    if let Ok(mut guard) = LOGGER.lock() {
        *guard = Some(logger);
    }
}

fn log(level: Level, msg: &str) {
    if let Ok(mut guard) = LOGGER.lock() {
        if let Some(ref mut logger) = *guard {
            logger.write(level, msg);
        } else {
            eprintln!("[{}] {}", level.as_str(), msg);
        }
    }
}

pub fn debug(msg: &str) { log(Level::Debug, msg); }
pub fn info(msg: &str) { log(Level::Info, msg); }
pub fn warn(msg: &str) { log(Level::Warn, msg); }
pub fn error(msg: &str) { log(Level::Error, msg); }