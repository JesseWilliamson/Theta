//! Panic reporting: a panic is a bug, and it must leave a trace the user can attach to a report.
//!
//! The hook writes the message, location and a forced backtrace to
//! `<config dir>/logs/panic-<unix millis>.log` and to stderr, then queues a notice. A panic on
//! a background task (the I/O runtime, the executor) does not end the app, and the window shows
//! the notice with the log path; a panic on the main thread ends the process as before, but
//! the log is already on disk by then.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A panic that has been logged and not yet shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub message: String,
    pub log: Option<PathBuf>,
}

static PENDING: Mutex<Vec<Report>> = Mutex::new(Vec::new());

/// Install the hook. Logs go to `<dir>/logs`.
pub fn install(dir: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let text = format!(
            "thread '{thread}' panicked at {location}:\n{message}\n\nstack backtrace:\n{backtrace}\n"
        );
        eprintln!("{text}");
        let log = write_log(&dir, &text).ok();
        if let Ok(mut pending) = PENDING.lock() {
            pending.push(Report { message: format!("{message} ({location})"), log });
        }
    }));
}

fn write_log(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    // Two panics in the same millisecond must not overwrite each other.
    let mut path = logs.join(format!("panic-{stamp}.log"));
    let mut n = 1;
    while path.exists() {
        path = logs.join(format!("panic-{stamp}-{n}.log"));
        n += 1;
    }
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Panics logged since the last call.
pub fn take_reports() -> Vec<Report> {
    PENDING.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default()
}

/// The sentence shown in the window.
pub fn notice_text(r: &Report) -> String {
    match &r.log {
        Some(p) => {
            format!("Quarry hit an internal error: {}. Details were saved to {}", r.message, p.display())
        }
        None => format!("Quarry hit an internal error: {}", r.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_are_written_without_overwriting_each_other() {
        let d = tempfile::tempdir().unwrap();
        let a = write_log(d.path(), "one").unwrap();
        let b = write_log(d.path(), "two").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(a).unwrap(), "one");
        assert!(b.starts_with(d.path().join("logs")));
    }

    #[test]
    fn the_hook_records_a_report_and_a_log_file() {
        let d = tempfile::tempdir().unwrap();
        // The hook is process-global; restore the previous one afterwards.
        let old = std::panic::take_hook();
        install(d.path().to_path_buf());
        let r = std::panic::catch_unwind(|| panic!("boom from a test"));
        let _ = std::panic::take_hook();
        std::panic::set_hook(old);
        assert!(r.is_err());
        let reports: Vec<Report> =
            take_reports().into_iter().filter(|r| r.message.contains("boom from a test")).collect();
        assert_eq!(reports.len(), 1);
        let log = reports[0].log.clone().expect("a log file");
        let body = std::fs::read_to_string(&log).unwrap();
        assert!(body.contains("boom from a test") && body.contains("stack backtrace"));
        assert!(notice_text(&reports[0]).contains(&log.display().to_string()));
    }
}
