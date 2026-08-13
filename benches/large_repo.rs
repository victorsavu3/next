//! Large-repository performance benchmark for the archiving design.
//!
//! Builds a synthetic post-archiving repository — a small active tier of
//! individual task files plus a large warm tier packed into segments — and
//! measures the hot paths against the design budgets (scaled 1M targets):
//!
//! | operation                        | budget at 10⁶ tasks |
//! |----------------------------------|---------------------|
//! | single edit (save + git commit)  | < 100 ms            |
//! | filtered query                   | < 1 s               |
//! | full-text search                 | < 1 s               |
//! | incremental reconcile, ≤200 files| < 10 s              |
//! | full cache rebuild               | < 10 min            |
//!
//! Tasks carry realistic prose (a description and ~160 words of notes), which
//! matters for more than fidelity: the full-text index stores a second copy of
//! that text, so a corpus of empty tasks would report a cache cost no real
//! repository ever sees — and would hide how search scales. The search
//! benchmarks caught a correlated-subquery formulation that cost 2.4 s for a
//! common term at 5 000 tasks, which a small real repository showed as 0.02 s.
//!
//! Run with `cargo bench --bench large_repo`. `NEXT_BENCH_TASKS` sets the
//! total task count (default 5 000 so a run stays under a minute; set
//! 1000000 to exercise the real target). `NEXT_BENCH_STRICT=1` makes budget
//! violations exit non-zero. The budgets are checked at every size — they
//! are worst-case bounds for 10⁶, so smaller runs must pass easily.

use std::path::Path;
use std::time::{Duration, Instant};

use next::core::domain::task::Task;
use next::core::storage::archive::{write_segment, ArchivedTask};
use next::core::storage::{CachedStore, GitBackend, TomlStore};
use next::core::store::{Store, TaskQuery, VcsBackend};

fn git(dir: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args).current_dir(dir);
    cmd.stdout(std::process::Stdio::null());
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        cmd.env_remove(var);
    }
    assert!(
        cmd.status().expect("spawn git").success(),
        "git {args:?} failed"
    );
}

/// First day of the calendar month `months_ago` before 2026-01 — distinct
/// months yield distinct segment paths.
fn month_anchor(months_ago: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() - chrono::Months::new(months_ago)
}

/// The vocabulary the synthetic prose is drawn from.
///
/// Deterministic and small on purpose: a term's hit rate is then a known
/// fraction of the corpus, so a search benchmark measures the index rather
/// than the luck of a random draw. `needle` appears in roughly 1 task in 200,
/// which is the selective-search case worth timing.
const VOCAB: &[&str] = &[
    "cache",
    "segment",
    "archive",
    "filter",
    "query",
    "index",
    "storage",
    "commit",
    "manifest",
    "pruned",
    "reconcile",
    "scoring",
    "urgency",
    "deadline",
    "recurrence",
    "plugin",
];

/// Description and notes for synthetic task `i`.
///
/// Real task notes here run to hundreds of words, and the whole point of
/// measuring is that the index copies this text — so a benchmark over empty
/// tasks would report a cache cost that no real repository would ever see.
fn prose(i: usize) -> (String, String) {
    let mut description = String::new();
    for w in 0..40 {
        description.push_str(VOCAB[(i + w) % VOCAB.len()]);
        description.push(' ');
    }
    let mut notes = String::with_capacity(1024);
    for w in 0..160 {
        notes.push_str(VOCAB[(i * 7 + w) % VOCAB.len()]);
        notes.push(' ');
        if w % 20 == 19 {
            notes.push('\n');
        }
    }
    // One term in ~200 tasks, so a selective search has real work to do.
    if i.is_multiple_of(200) {
        notes.push_str("needle");
    }
    (description, notes)
}

/// Builds the synthetic repository following the design's workload model
/// (§3 of the proposal): ~1% of tasks are open at any moment — the archive
/// pass is what keeps the active tier this small — so they live as
/// individual files (10% of them tagged for the filtered query), capped at
/// 200 minimum so the reconcile benchmark always has files to touch. The
/// rest are archived into month segments of 1000.
fn build_repo(root: &Path, total: usize) -> (Vec<Task>, usize) {
    let tasks_dir = root.join("tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();

    let active_count = (total / 100).max(200);
    let mut active = Vec::with_capacity(active_count);
    for i in 0..active_count {
        let mut t = Task::new(format!("Active task {i}"));
        if i % 10 == 0 {
            t.tags = vec!["@bench/hot".into()];
        }
        let (description, notes) = prose(i);
        t.description = Some(description);
        t.notes = Some(notes);
        std::fs::write(
            next::core::storage::task_path(root, &t),
            toml::to_string_pretty(&t).unwrap(),
        )
        .unwrap();
        active.push(t);
    }

    let archived_count = total.saturating_sub(active_count);
    let mut entries: Vec<ArchivedTask> = Vec::with_capacity(1000);
    let mut month = 0u32;
    let mut seg_in_month = 1u32;
    let now = chrono::Utc::now();
    for i in 0..archived_count {
        let mut t = Task::new(format!("Archived task {i}"));
        let (description, notes) = prose(i);
        t.description = Some(description);
        t.notes = Some(notes);
        // Spread completions over months, ~3000/month → 3 segments per month.
        t.mark_done(month_anchor(month));
        entries.push(ArchivedTask {
            created_at: Some(now),
            updated_at: Some(now),
            task: t,
        });
        if entries.len() == 1000 {
            write_segment(
                &root.join(next::core::storage::archive::segment_rel_path(
                    month_anchor(month),
                    seg_in_month,
                )),
                std::mem::take(&mut entries),
            )
            .unwrap();
            seg_in_month += 1;
            if seg_in_month > 3 {
                month += 1;
                seg_in_month = 1;
            }
        }
    }
    if !entries.is_empty() {
        write_segment(
            &root.join(next::core::storage::archive::segment_rel_path(
                month_anchor(month),
                seg_in_month,
            )),
            entries,
        )
        .unwrap();
    }

    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "bench corpus"]);
    (active, archived_count)
}

struct Budget {
    name: &'static str,
    took: Duration,
    limit: Duration,
}

fn main() {
    let total: usize = std::env::var("NEXT_BENCH_TASKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000);
    let strict = std::env::var("NEXT_BENCH_STRICT").as_deref() == Ok("1");

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "bench@bench"]);
    git(root, &["config", "user.name", "Bench"]);

    eprintln!("building corpus: {total} tasks …");
    let t0 = Instant::now();
    let (active, archived_count) = build_repo(root, total);
    eprintln!(
        "corpus ready in {:.1?} ({} active files, {} archived in segments)",
        t0.elapsed(),
        active.len(),
        archived_count
    );

    let mut results: Vec<Budget> = Vec::new();

    // ── Full rebuild: fresh cache over the whole corpus ─────────────────────
    let vcs = GitBackend::open(root).unwrap();
    let head = vcs.head_hash().unwrap();
    let t = Instant::now();
    let inner = TomlStore::open(root.to_path_buf(), root.join("state.toml")).unwrap();
    let mut store = CachedStore::open(inner, root.join(".next.db"), &head).unwrap();
    results.push(Budget {
        name: "full rebuild",
        took: t.elapsed(),
        limit: Duration::from_secs(600),
    });

    // ── Incremental reconcile: 200 changed files in one commit ──────────────
    let changed = active.iter().take(200).collect::<Vec<_>>();
    let mut paths = Vec::new();
    for t in &changed {
        let mut edited = (*t).clone();
        edited.notes = Some("reconcile me".into());
        let p = next::core::storage::task_path(root, &edited);
        std::fs::write(&p, toml::to_string_pretty(&edited).unwrap()).unwrap();
        paths.push(p);
    }
    vcs.commit(&paths, "bench: 200 external changes").unwrap();
    let new_head = vcs.head_hash().unwrap();
    let t = Instant::now();
    store.after_pull(&new_head).unwrap();
    results.push(Budget {
        name: "incremental reconcile (200)",
        took: t.elapsed(),
        limit: Duration::from_secs(10),
    });

    // ── Single edit: save one task + commit its path ────────────────────────
    let mut task = active[0].clone();
    task.title = "Edited by the bench".into();
    let t = Instant::now();
    store.save_task(&task).unwrap();
    vcs.commit(
        std::slice::from_ref(&next::core::storage::task_path(root, &task)),
        "bench: single edit",
    )
    .unwrap();
    results.push(Budget {
        name: "single edit (save+commit)",
        took: t.elapsed(),
        limit: Duration::from_millis(100),
    });

    // ── Filtered query: tag + status pushdown, first page ───────────────────
    let t = Instant::now();
    let page = store
        .query_tasks(&TaskQuery {
            required_tags: vec!["@bench".into()],
            ..TaskQuery::default()
        })
        .unwrap();
    results.push(Budget {
        name: "filtered query (page 1)",
        took: t.elapsed(),
        limit: Duration::from_secs(1),
    });
    assert!(page.total > 0, "filtered query must match the tagged tasks");

    // ── Archived query: newest completions, first page ──────────────────────
    let t = Instant::now();
    let archived = store
        .query_tasks(&TaskQuery {
            archived: true,
            ..TaskQuery::default()
        })
        .unwrap();
    results.push(Budget {
        name: "archived query (page 1)",
        took: t.elapsed(),
        limit: Duration::from_secs(1),
    });
    assert_eq!(archived.total as usize, archived_count);

    // ── Full-text search: the FTS index, over both tiers ────────────────────
    //
    // `needle` is in ~1 task in 200, so this is the selective case. It is also
    // the query shape that decides whether the archive tier can be searched at
    // all: before the index it was a scan of every deserialised row.
    let search = |query: &str| TaskQuery {
        filter: Some(next::core::store::QueryFilter::new(
            next::core::domain::filter_expr::parse(query).unwrap(),
            chrono::Local::now().date_naive(),
        )),
        ..TaskQuery::default()
    };

    let t = Instant::now();
    let found = store.query_tasks(&search("needle")).unwrap();
    results.push(Budget {
        name: "search, selective (page 1)",
        took: t.elapsed(),
        limit: Duration::from_secs(1),
    });
    assert!(found.total > 0, "the needle must be findable");

    // A common term matches most of the corpus — the pagination case, where
    // the index has to rank far more rows than fit on a page.
    let t = Instant::now();
    let common = store.query_tasks(&search("cache")).unwrap();
    results.push(Budget {
        name: "search, common term (page 1)",
        took: t.elapsed(),
        limit: Duration::from_secs(1),
    });
    assert!(common.total > 0, "a vocabulary word must match");

    // Prefix search cannot use a plain term index, so it is timed separately.
    let t = Instant::now();
    store.query_tasks(&search("archiv*")).unwrap();
    results.push(Budget {
        name: "search, prefix (page 1)",
        took: t.elapsed(),
        limit: Duration::from_secs(1),
    });

    // Searching the ARCHIVE is the payoff: this reaches text in segments that
    // a working-tree grep cannot see at all.
    let t = Instant::now();
    let archived_hits = store
        .query_tasks(&TaskQuery {
            archived: true,
            ..search("needle")
        })
        .unwrap();
    results.push(Budget {
        name: "search, archived tier",
        took: t.elapsed(),
        limit: Duration::from_secs(1),
    });
    assert!(
        archived_hits.total > 0,
        "archived text must be searchable — that is the point of indexing it"
    );

    // ── Cache footprint ─────────────────────────────────────────────────────
    //
    // The full-text index stores a second copy of every task's prose, so it is
    // the largest single cost of making search indexed. Reported rather than
    // budgeted: what counts as too big is a judgement about the machine, not a
    // number the benchmark can assert.
    drop(store);
    let cache_bytes = std::fs::metadata(root.join(".next.db"))
        .map(|m| m.len())
        .unwrap_or(0);
    let fts_bytes = {
        let conn = rusqlite::Connection::open(root.join(".next.db")).unwrap();
        conn.query_row(
            "SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name LIKE 'task_fts%'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(-1)
    };
    let mib = |b: f64| b / (1024.0 * 1024.0);
    println!("\ncache: {:.1} MiB total", mib(cache_bytes as f64));
    if fts_bytes >= 0 {
        println!(
            "  full-text index: {:.1} MiB ({:.0}% of the cache, {:.0} bytes/task)",
            mib(fts_bytes as f64),
            100.0 * fts_bytes as f64 / cache_bytes as f64,
            fts_bytes as f64 / total as f64,
        );
    }

    // ── Report ───────────────────────────────────────────────────────────────
    println!("\n{total} tasks — budgets are the 1M worst-case bounds:");
    println!("{:<30} {:>12} {:>10}", "operation", "took", "budget");
    let mut failed = false;
    for b in &results {
        let ok = b.took <= b.limit;
        failed |= !ok;
        println!(
            "{:<30} {:>12} {:>10} {}",
            b.name,
            format!("{:.2?}", b.took),
            format!("{:.0?}", b.limit),
            if ok { "ok" } else { "OVER BUDGET" }
        );
    }
    if failed && strict {
        std::process::exit(1);
    }
}
