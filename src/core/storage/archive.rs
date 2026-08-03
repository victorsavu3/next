//! Warm-tier archive segments.
//!
//! Closed tasks past the archive threshold move out of `tasks/*.toml` into
//! append-once TOML segment files keyed by completion month and capped in
//! size: `archive/<YYYY>/<MM>-<NNN>.toml`. Segments stay in the checkout, so
//! a cache rebuild parses files and never needs git history; each entry
//! carries its git-derived timestamps frozen at archive time, making the
//! archive self-contained.
//!
//! Segment bytes are deterministic — entries sorted by `(completed_at, id)`,
//! canonical serialization — so two machines archiving the same tasks
//! produce identical files and git merges them silently.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::domain::task::Task;
use crate::core::error::{Result, TaskError};

/// Committed archive policy, `<repo>/config/archive.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveConfig {
    /// Closed tasks whose reference date is older than this move to the
    /// warm tier.
    pub archive_after_days: u32,
    /// A segment is sealed once it holds this many tasks; the next task
    /// starts a new segment file.
    pub segment_max_tasks: usize,
    /// Run the archive pass automatically during sync (at most once a day).
    pub auto: bool,
    /// Cold tier: segments whose newest completion is older than this are
    /// pruned from the checkout (recoverable through git blobs via the
    /// manifest). `None` (the default) disables pruning.
    pub prune_after_days: Option<u32>,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            archive_after_days: 180,
            segment_max_tasks: 1000,
            auto: true,
            prune_after_days: None,
        }
    }
}

/// Path of the committed archive policy file.
pub fn archive_config_path(root: &Path) -> PathBuf {
    root.join("config").join("archive.toml")
}

/// Loads `<root>/config/archive.toml`; absent or unparsable files fall back
/// to the defaults (with a warning for the latter), like `load_scoring`.
pub fn load_archive_config(root: &Path) -> ArchiveConfig {
    let path = archive_config_path(root);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ArchiveConfig::default(),
    };
    match toml::from_str(&content) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!(
                "failed to parse {}: {e}; using default archive policy",
                path.display()
            );
            ArchiveConfig::default()
        }
    }
}

/// One archived task: the full task plus its git-derived timestamps frozen
/// at archive time, so archived tasks never need git history again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedTask {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub task: Task,
}

/// On-disk segment shape: a TOML array-of-tables named `task`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SegmentFile {
    #[serde(default)]
    task: Vec<ArchivedTask>,
}

/// Whether a repo-relative path (forward slashes) is an archive segment.
pub fn is_segment_path(rel_path: &str) -> bool {
    rel_path.starts_with("archive/") && rel_path.ends_with(".toml")
}

/// Repo-relative path of segment `n` for the month of `date`:
/// `archive/2026/07-003.toml`. `n` is 1-based.
pub fn segment_rel_path(date: chrono::NaiveDate, n: u32) -> String {
    use chrono::Datelike as _;
    format!(
        "archive/{:04}/{:02}-{:03}.toml",
        date.year(),
        date.month(),
        n
    )
}

/// Reads a segment file into its entries. A missing file is an empty segment.
pub fn read_segment(path: &Path) -> Result<Vec<ArchivedTask>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(TaskError::Other(format!("read {}: {e}", path.display()))),
    };
    parse_segment(&content)
}

/// Parses segment content (e.g. fetched from a git blob for the cold tier).
pub fn parse_segment(content: &str) -> Result<Vec<ArchivedTask>> {
    let file: SegmentFile =
        toml::from_str(content).map_err(|e| TaskError::Other(format!("parse segment: {e}")))?;
    Ok(file.task)
}

// ── Cold tier: the pruned-segment manifest ─────────────────────────────────

/// One pruned segment: enough to recover its bytes from the object store
/// with a single blob read — never a history walk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrunedSegment {
    /// Repo-relative path the segment had in the checkout.
    pub path: String,
    /// Blob SHA of the segment content at prune time.
    pub blob: String,
    /// Number of tasks inside, for reporting.
    pub tasks: usize,
}

/// Repo-relative path of the append-only manifest.
pub const MANIFEST_REL_PATH: &str = "archive/pruned.jsonl";

/// Reads the manifest. Later lines win per path (a segment restored by a
/// resurrection and pruned again appends a fresh line), so the result holds
/// one entry per path, in path order. Unparsable lines are skipped — the
/// file is union-merged and a torn line must not poison the archive.
pub fn read_manifest(root: &Path) -> Result<Vec<PrunedSegment>> {
    let path = root.join(MANIFEST_REL_PATH);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(TaskError::Other(format!("read {}: {e}", path.display()))),
    };
    let mut by_path = std::collections::BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<PrunedSegment>(line) {
            by_path.insert(entry.path.clone(), entry);
        }
    }
    Ok(by_path.into_values().collect())
}

/// Appends `entries` to the manifest (creating it if needed).
pub fn append_manifest(root: &Path, entries: &[PrunedSegment]) -> Result<()> {
    use std::io::Write as _;
    let path = root.join(MANIFEST_REL_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| TaskError::Other(format!("open {}: {e}", path.display())))?;
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|e| TaskError::Other(format!("serialize manifest line: {e}")))?;
        writeln!(file, "{line}")
            .map_err(|e| TaskError::Other(format!("append {}: {e}", path.display())))?;
    }
    Ok(())
}

/// Ensures `.gitattributes` union-merges the manifest, so two machines
/// pruning concurrently never conflict on it. Returns the attributes path
/// when it was created or extended (the caller commits it).
pub fn ensure_manifest_gitattributes(root: &Path) -> Result<Option<PathBuf>> {
    let attr_line = format!("{MANIFEST_REL_PATH} merge=union");
    let path = root.join(".gitattributes");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == attr_line) {
        return Ok(None);
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&attr_line);
    content.push('\n');
    super::toml_store::atomic_write(&path, &content)?;
    Ok(Some(path))
}

/// Writes `entries` to `path` deterministically: sorted by
/// `(completed_at, id)`, canonical serialization, atomic replace. An empty
/// `entries` removes the file.
pub fn write_segment(path: &Path, mut entries: Vec<ArchivedTask>) -> Result<()> {
    if entries.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(TaskError::Other(format!("remove {}: {e}", path.display()))),
        }
    }
    entries.sort_by_key(|e| (e.task.completed_at, e.task.id));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TaskError::Other(format!("create {}: {e}", parent.display())))?;
    }
    let content = toml::to_string_pretty(&SegmentFile { task: entries })
        .map_err(|e| TaskError::Other(format!("serialize segment: {e}")))?;
    super::toml_store::atomic_write(path, &content)
}

/// All segment files under `<root>/archive/`, as (repo-relative path,
/// absolute path) pairs sorted by relative path.
pub fn segment_paths(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let archive_dir = root.join("archive");
    let mut result = Vec::new();
    if !archive_dir.exists() {
        return Ok(result);
    }
    collect_segments(&archive_dir, root, &mut result)?;
    result.sort();
    Ok(result)
}

fn collect_segments(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_segments(&path, root, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((rel_str, path.clone()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn archived(title: &str, completed: Option<NaiveDate>) -> ArchivedTask {
        let mut task = Task::new(title);
        if let Some(d) = completed {
            task.mark_done(d);
        }
        ArchivedTask {
            created_at: DateTime::from_timestamp(1_700_000_000, 0),
            updated_at: DateTime::from_timestamp(1_750_000_000, 0),
            task,
        }
    }

    #[test]
    fn segment_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("archive/2026/01-001.toml");
        let entries = vec![
            archived("One", NaiveDate::from_ymd_opt(2026, 1, 5)),
            archived("Two", NaiveDate::from_ymd_opt(2026, 1, 9)),
        ];
        write_segment(&path, entries.clone()).unwrap();

        let loaded = read_segment(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].task.title, "One");
        assert_eq!(loaded[0].created_at, entries[0].created_at);
        assert_eq!(loaded[0].task.completed_at, entries[0].task.completed_at);
    }

    #[test]
    fn segment_bytes_are_deterministic() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = archived("A", NaiveDate::from_ymd_opt(2026, 2, 1));
        let b = archived("B", NaiveDate::from_ymd_opt(2026, 2, 3));

        let p1 = dir.path().join("one.toml");
        let p2 = dir.path().join("two.toml");
        write_segment(&p1, vec![a.clone(), b.clone()]).unwrap();
        write_segment(&p2, vec![b, a]).unwrap();

        assert_eq!(
            std::fs::read_to_string(&p1).unwrap(),
            std::fs::read_to_string(&p2).unwrap(),
            "insertion order must not affect segment bytes"
        );
    }

    #[test]
    fn empty_segment_removes_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("seg.toml");
        write_segment(
            &path,
            vec![archived("X", NaiveDate::from_ymd_opt(2026, 3, 1))],
        )
        .unwrap();
        assert!(path.exists());
        write_segment(&path, Vec::new()).unwrap();
        assert!(!path.exists());
        // Removing an already-absent segment is fine.
        write_segment(&path, Vec::new()).unwrap();
    }

    #[test]
    fn read_missing_segment_is_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_segment(&dir.path().join("nope.toml"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn segment_rel_path_layout() {
        let d = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
        assert_eq!(segment_rel_path(d, 1), "archive/2026/07-001.toml");
        assert_eq!(segment_rel_path(d, 12), "archive/2026/07-012.toml");
        assert!(is_segment_path(&segment_rel_path(d, 1)));
        assert!(!is_segment_path("tasks/foo.toml"));
    }

    #[test]
    fn segment_paths_walk() {
        let dir = tempfile::TempDir::new().unwrap();
        let p1 = dir.path().join("archive/2025/12-001.toml");
        let p2 = dir.path().join("archive/2026/01-001.toml");
        write_segment(
            &p1,
            vec![archived("A", NaiveDate::from_ymd_opt(2025, 12, 1))],
        )
        .unwrap();
        write_segment(
            &p2,
            vec![archived("B", NaiveDate::from_ymd_opt(2026, 1, 1))],
        )
        .unwrap();

        let found = segment_paths(dir.path()).unwrap();
        let rels: Vec<_> = found.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(
            rels,
            vec!["archive/2025/12-001.toml", "archive/2026/01-001.toml"]
        );
    }

    #[test]
    fn config_defaults_and_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = load_archive_config(dir.path());
        assert_eq!(cfg.archive_after_days, 180);
        assert_eq!(cfg.segment_max_tasks, 1000);
        assert!(cfg.auto);

        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(
            archive_config_path(dir.path()),
            "archive_after_days = 30\nauto = false\n",
        )
        .unwrap();
        let cfg = load_archive_config(dir.path());
        assert_eq!(cfg.archive_after_days, 30);
        assert_eq!(cfg.segment_max_tasks, 1000, "omitted fields keep defaults");
        assert!(!cfg.auto);
    }
}
