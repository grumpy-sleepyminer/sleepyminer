use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn init(verbose: bool) {
    VERBOSE.store(verbose, Ordering::Relaxed);
    let level = if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(level);
}

static LOGGER: SleepyLogger = SleepyLogger;

struct SleepyLogger;

fn category_from_target(target: &str) -> (&'static str, &'static str) {
    if target.contains("stratum") || target.contains("connection") {
        ("\x1b[36m", "net")
    } else if target.contains("worker") {
        ("\x1b[32m", "cpu")
    } else if target.contains("miner") {
        ("\x1b[1;37m", "miner")
    } else if target.contains("scaling") || target.contains("activity") {
        ("\x1b[33m", "scale")
    } else if target.contains("donation") {
        ("\x1b[35m", "pool")
    } else if target.contains("randomx") {
        ("\x1b[34m", "rx")
    } else if target.contains("benchmark") {
        ("\x1b[33m", "bench")
    } else if target.contains("service") {
        ("\x1b[90m", "svc")
    } else {
        ("\x1b[37m", "sleepy")
    }
}

fn level_color(level: Level) -> &'static str {
    match level {
        Level::Error => "\x1b[1;31m",
        Level::Warn => "\x1b[1;33m",
        Level::Info => "",
        Level::Debug => "\x1b[90m",
        Level::Trace => "\x1b[90m",
    }
}

impl Log for SleepyLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if VERBOSE.load(Ordering::Relaxed) {
            metadata.level() <= Level::Debug
        } else {
            metadata.level() <= Level::Info
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let now = chrono_local_time();
        let (cat_color, cat_name) = category_from_target(record.target());
        let lvl_color = level_color(record.level());
        let reset = "\x1b[0m";

        let prefix = match record.level() {
            Level::Error => "\x1b[1;31m✗\x1b[0m ",
            Level::Warn => "\x1b[1;33m⚠\x1b[0m ",
            _ => "",
        };

        let _ = writeln!(
            std::io::stderr(),
            "\x1b[90m{}\x1b[0m  {}{:<6}{}  {}{}{}",
            now,
            cat_color,
            cat_name,
            reset,
            prefix,
            lvl_color,
            record.args(),
        );
    }

    fn flush(&self) {}
}

fn chrono_local_time() -> String {
    use libc::{localtime_r, time, time_t, tm};
    unsafe {
        let mut t: time_t = 0;
        time(&mut t);
        let mut local: tm = std::mem::zeroed();
        localtime_r(&t, &mut local);
        format!(
            "{:02}:{:02}:{:02}",
            local.tm_hour, local.tm_min, local.tm_sec
        )
    }
}
