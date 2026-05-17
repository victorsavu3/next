use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use chrono::Utc;

const MAX_BYTES: u64 = 1024 * 1024; // 1 MB before rotation

/// Persistent append-only command log written to `<repo>/next.log`.
///
/// Entries are plain text: `2026-05-17T12:00:00Z INFO  [cmd] message`.
/// When the file exceeds 1 MB it is renamed to `next.log.1` (overwriting any
/// previous backup) and a new file is started.
///
/// All write failures are silently swallowed — a failed log write must never
/// abort a command.
pub struct Logger {
    path: PathBuf,
}

impl Logger {
    pub fn new(repo_root: &Path) -> Self {
        Self {
            path: repo_root.join("next.log"),
        }
    }

    pub fn info(&self, command: &str, message: &str) {
        self.append("INFO ", command, message);
    }

    pub fn error(&self, command: &str, message: &str) {
        self.append("ERROR", command, message);
    }

    fn append(&self, level: &str, command: &str, message: &str) {
        self.maybe_rotate();
        let ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let line = format!("{ts} {level} [{command}] {message}\n");
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn maybe_rotate(&self) {
        if fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) < MAX_BYTES {
            return;
        }
        let backup = self.path.with_extension("log.1");
        let _ = fs::rename(&self.path, backup);
    }
}
