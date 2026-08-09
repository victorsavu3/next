//! Differential tests for the SQL filter pushdown (stage S2).
//!
//! Two implementations answer one question: `CachedStore::query_tasks`
//! compiles part of the expression into SQLite, and the default
//! `Store::query_tasks` filters in memory. The in-memory one is the reference
//! — the SQL must agree with it, not the other way round — so every test here
//! runs both and compares.
//!
//! The corpus deliberately spans both archive tiers. Full-fidelity archived
//! filtering makes the SQLite cache load-bearing for the *correctness* of a
//! user-visible query rather than only for its speed, so "the cache has the
//! rows" is a thing to test, not to assume.

mod common;

use std::collections::BTreeSet;

use chrono::NaiveDate;
use next::core::archiver::run_archive_pass;
use next::core::domain::filter_expr::parse;
use next::core::domain::task::{Priority, Task};
use next::core::store::{QueryFilter, Store, TaskQuery};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn today() -> NaiveDate {
    d(2026, 7, 15)
}

fn add_committed(env: &mut common::TestEnv, task: &Task) {
    env.ctx
        .repo
        .transaction(|store, vcs, root| {
            store.save_task(task)?;
            vcs.commit(
                &[next::core::storage::task_path(root, task)],
                &format!("next: add {:?}", task.title),
            )?;
            Ok(())
        })
        .unwrap();
}

/// Every expression the differential runs. Chosen to cross the pushdown's
/// seams rather than to be exhaustive: each one is either fully pushed, fully
/// residual, or — the interesting case — a mix that forces the compiler to
/// widen.
const EXPRESSIONS: &[&str] = &[
    // Fully pushable.
    "",
    "+@work",
    "+@work/backend",
    "-@work",
    "tag:@work,@home",
    // `LIKE` folds ASCII case and treats `_` as a wildcard — and `_` is a
    // legal tag character — so the old descendant test matched tags the
    // evaluator rejects while claiming to be exact.
    "+@WORK",
    "+@Work/Backend",
    "+a_b",
    "+@work/back",
    "status:done",
    "status:done,cancelled",
    // The evaluator parses enum values case-insensitively and accepts aliases;
    // the column stores one canonical spelling. Binding the raw text into a
    // case-sensitive comparison dropped every match, and the atom is exact so
    // nothing re-checked it. These four are the regression guard.
    "status:DONE",
    "status:canceled",
    "priority:HIGH",
    "priority:med",
    "priority:high",
    "priority>=medium",
    "priority<high",
    "slug:alpha",
    "assignee:alice",
    "user:alice",
    "has:due",
    "no:due",
    "has:assignee",
    "has:tag",
    "is:closed",
    "is:archived",
    "is:assigned",
    "is:overdue",
    "completed<2026-01-01",
    "completed:2025-01-01..2025-12-31",
    "due:2026-08-01",
    // Fully residual (nothing to push).
    "alpha",
    "\"the cold\"",
    "arch*",
    "title:beta",
    "notes:measured",
    "data.estimate:3",
    "data.estimate>1",
    "has:data.estimate",
    "is:recurring",
    "has:description",
    // Mixed — the cases where a naive compiler pushes a subset.
    "+@work alpha",
    "+@work or alpha",
    "not alpha",
    "not (+@work and alpha)",
    "not (+@work or alpha)",
    "+@work or (status:done and alpha)",
    "(+@work or alpha) and priority:high",
    "not is:recurring and +@work",
    "not data.estimate:3",
    "+@work and not alpha",
    // Boolean shapes over pushable atoms only, so the whole thing is pushed
    // including the negations.
    // Negation over a NULLABLE column: SQL's `NOT NULL` is NULL, not true, so
    // a plain `NOT (col = ?)` dropped every task where the column is unset —
    // which the evaluator matches. These are the guard for that.
    "not +@work",
    "not slug:alpha",
    "not assignee:alice",
    "not due:2026-08-01",
    "not due<2026-08-01",
    "not (status:done or priority:low)",
    "+@work and (status:done or priority:high)",
    "(+@work or +@home) and not is:closed",
];

/// The ids a query returns, order-insensitive — pagination order is covered by
/// its own tests; this is about *which* tasks match.
fn ids(page: &next::core::store::Page<Task>) -> BTreeSet<String> {
    page.items.iter().map(|t| t.id.to_string()).collect()
}

/// Runs one expression down both paths and asserts they agree.
fn assert_paths_agree(sql_store: &dyn Store, reference: &[Task], query: &str, archived: bool) {
    let expr = parse(query).unwrap_or_else(|e| panic!("{query}: {e}"));
    let q = TaskQuery {
        archived,
        filter: Some(QueryFilter::new(expr, today())),
        ..TaskQuery::unpaginated()
    };

    let from_sql = sql_store.query_tasks(&q).unwrap();

    // The reference: the same predicate applied in memory, with no SQL
    // anywhere near it.
    let expected: BTreeSet<String> = reference
        .iter()
        .filter(|t| q.expression_matches(t))
        .map(|t| t.id.to_string())
        .collect();

    assert_eq!(
        ids(&from_sql),
        expected,
        "\nquery: {query}\narchived: {archived}\nSQL path and in-memory reference disagree"
    );
    assert_eq!(
        from_sql.total as usize,
        expected.len(),
        "\nquery: {query}\narchived: {archived}\ntotal must count the matches, not the candidates"
    );
}

/// Builds a corpus with active tasks, a warm archive segment and a pruned cold
/// segment, and returns the store plus the archived tasks as plain values.
fn corpus(env: &mut common::TestEnv) -> (Vec<Task>, Vec<Task>) {
    let root = env.ctx.repo.repo_root.clone();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 180\nprune_after_days = 400\n",
    )
    .unwrap();

    let mut active: Vec<Task> = Vec::new();
    let mut archived: Vec<Task> = Vec::new();

    // ── Active tier ──────────────────────────────────────────────────────────
    let mut alpha = Task::new("Alpha rebuild");
    alpha.slug = Some("alpha".into());
    alpha.tags = vec!["@work/backend".into(), "bug".into()];
    alpha.priority = Priority::High;
    alpha.due = Some(d(2026, 8, 1));
    alpha.assignee = Some("alice".into());
    alpha.description = Some("The cold tier drifts.".into());
    alpha.data.insert("estimate".into(), serde_json::json!(3));
    active.push(alpha);

    let mut beta = Task::new("Beta groundwork");
    beta.slug = Some("beta".into());
    beta.tags = vec!["@home".into()];
    beta.priority = Priority::Low;
    beta.notes = Some("Measured on the real repository.".into());
    active.push(beta);

    let mut gamma = Task::new("Gamma overdue thing");
    gamma.slug = Some("gamma".into());
    gamma.tags = vec!["@work".into()];
    gamma.due = Some(d(2026, 1, 1));
    gamma.recurrence = Some(next::core::domain::task::Recurrence::Completion {
        interval_days: 7,
        snap: None,
    });
    active.push(gamma);

    // Untagged and unassigned, so `user:` and the tag atoms have a negative case.
    active.push(Task::new("Delta plain"));

    // `_` is a legal tag character and a `LIKE` wildcard. These two exist so
    // that `+a_b` has both a task it must match and one it must not: with the
    // old translation, `a_b` matched `axb/c` as well.
    let mut underscore = Task::new("Epsilon underscore tag");
    underscore.tags = vec!["a_b/c".into()];
    active.push(underscore);

    let mut wildcarded = Task::new("Zeta wildcard decoy");
    wildcarded.tags = vec!["axb/c".into()];
    active.push(wildcarded);

    // ── Archive tier ─────────────────────────────────────────────────────────
    // Ancient enough to be pruned into the cold tier.
    let mut ancient = Task::new("Ancient archived work");
    ancient.slug = Some("ancient".into());
    ancient.tags = vec!["@work".into()];
    ancient.priority = Priority::High;
    ancient.assignee = Some("alice".into());
    ancient.data.insert("estimate".into(), serde_json::json!(5));
    ancient.mark_done(d(2025, 3, 1));
    archived.push(ancient);

    // Warm: archived but still in the checkout.
    let mut warm = Task::new("Warm archived alpha thing");
    warm.slug = Some("warm".into());
    warm.tags = vec!["@home/kitchen".into()];
    warm.description = Some("Archived while the cold tier was being built.".into());
    warm.mark_done(d(2025, 12, 1));
    archived.push(warm);

    let mut warm_cancelled = Task::new("Warm cancelled");
    warm_cancelled.tags = vec!["@work/backend".into()];
    warm_cancelled.mark_cancelled();
    warm_cancelled.completed_at = Some(d(2025, 12, 20));
    archived.push(warm_cancelled);

    for t in active.iter().chain(archived.iter()) {
        add_committed(env, t);
    }

    let outcome = run_archive_pass(&mut env.ctx.repo, today()).unwrap();
    assert_eq!(outcome.archived, archived.len(), "corpus must archive");
    assert!(
        !outcome.pruned.is_empty(),
        "corpus must include a pruned cold segment"
    );

    (active, archived)
}

#[test]
fn sql_and_in_memory_agree_on_the_active_tier() {
    let mut env = common::setup();
    let (active, _) = corpus(&mut env);
    let store = env.ctx.repo.store();
    for query in EXPRESSIONS {
        assert_paths_agree(store, &active, query, false);
    }
}

#[test]
fn sql_and_in_memory_agree_on_the_archived_tier() {
    // The payoff: `list --archived` now takes the whole grammar, over rows
    // that include a segment which is not in the working tree at all.
    let mut env = common::setup();
    let (_, archived) = corpus(&mut env);
    let store = env.ctx.repo.store();
    for query in EXPRESSIONS {
        assert_paths_agree(store, &archived, query, true);
    }
}

#[test]
fn a_rebuilt_cache_answers_the_same_queries() {
    // Archived filtering leans on the cache for correctness, so the rebuild
    // path has to reproduce every row — including the cold ones, which come
    // back from manifest blobs rather than from the checkout.
    let mut env = common::setup();
    let (_, archived) = corpus(&mut env);
    let root = env.ctx.repo.repo_root.clone();

    let inner =
        next::core::storage::TomlStore::open(root.clone(), root.join("state.toml")).unwrap();
    let vcs = next::core::storage::GitBackend::open(&root).unwrap();
    let head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
    let fresh =
        next::core::storage::CachedStore::open(inner, root.join(".next-fresh.db"), &head).unwrap();

    for query in EXPRESSIONS {
        assert_paths_agree(&fresh, &archived, query, true);
    }
}

#[test]
fn pagination_is_consistent_whether_or_not_the_query_was_pushed() {
    // A residual filter runs after SQL, so the naive implementation paginates
    // the candidates and then drops some of them — leaving short pages and a
    // total that counts rows nobody matched.
    let mut env = common::setup();
    let (active, _) = corpus(&mut env);
    let store = env.ctx.repo.store();

    for query in ["+@work", "+@work or alpha", "alpha"] {
        let expr = parse(query).unwrap();
        let expected: usize = active
            .iter()
            .filter(|t| {
                TaskQuery {
                    filter: Some(QueryFilter::new(expr.clone(), today())),
                    ..TaskQuery::default()
                }
                .expression_matches(t)
            })
            .count();

        let paged = store
            .query_tasks(&TaskQuery {
                filter: Some(QueryFilter::new(expr.clone(), today())),
                page: 1,
                page_size: 1,
                ..TaskQuery::default()
            })
            .unwrap();
        assert_eq!(
            paged.total as usize, expected,
            "{query}: total must be the match count, not the candidate count"
        );
        assert_eq!(
            paged.items.len(),
            expected.min(1),
            "{query}: a full page must be full"
        );

        // Walking the pages must visit every match exactly once.
        let mut seen = BTreeSet::new();
        for page in 1..=(expected.max(1) as u32) {
            let p = store
                .query_tasks(&TaskQuery {
                    filter: Some(QueryFilter::new(expr.clone(), today())),
                    page,
                    page_size: 1,
                    ..TaskQuery::default()
                })
                .unwrap();
            for t in p.items {
                assert!(seen.insert(t.id), "{query}: page {page} repeated a task");
            }
        }
        assert_eq!(seen.len(), expected, "{query}: pages missed a match");
    }
}

#[test]
fn a_filter_value_cannot_inject_sql() {
    let mut env = common::setup();
    let (_, _) = corpus(&mut env);
    let store = env.ctx.repo.store();

    // If the value were interpolated rather than bound, this drops the table
    // and every later query fails.
    for nasty in ["x'; DROP TABLE tasks; --", "' OR '1'='1", "%", "_", "\\"] {
        let expr = parse(&format!("slug:{nasty:?}")).unwrap();
        let page = store
            .query_tasks(&TaskQuery {
                filter: Some(QueryFilter::new(expr, today())),
                ..TaskQuery::unpaginated()
            })
            .unwrap();
        assert_eq!(page.total, 0, "{nasty:?} should match nothing");
    }

    // The table is still there and still populated.
    let all = store.query_tasks(&TaskQuery::unpaginated()).unwrap();
    assert!(all.total > 0, "the tasks table survived");
}

// ─── The surface ────────────────────────────────────────────────────────────
//
// The tests above prove the two store paths agree. These prove the archived
// tier actually reaches that machinery: before S2 it accepted `+tag` and `-tag`
// and nothing else.

/// Runs `list_tasks(archived: true)` with a filter, as an agent would.
fn archived_titles(env: &mut common::TestEnv, query: &str) -> Vec<String> {
    let result = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({ "archived": true, "filter_tokens": [query] }),
        &mut env.ctx.repo,
    )
    .unwrap_or_else(|e| panic!("{query}: {e}"));
    result["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|t| t["title"].as_str().expect("title").to_owned())
        .collect()
}

#[test]
fn the_archived_tier_accepts_the_whole_grammar() {
    let mut env = common::setup();
    corpus(&mut env);

    // Tags still work, as they always did — including the hierarchy, so
    // `@work` also picks up the task tagged `@work/backend`.
    let mut work = archived_titles(&mut env, "+@work");
    work.sort();
    assert_eq!(work, vec!["Ancient archived work", "Warm cancelled"]);
    assert_eq!(
        archived_titles(&mut env, "+@work/backend"),
        vec!["Warm cancelled"]
    );

    // And everything S2 adds: field predicates, search, booleans, negation.
    assert_eq!(
        archived_titles(&mut env, "status:cancelled"),
        vec!["Warm cancelled"]
    );
    assert_eq!(
        archived_titles(&mut env, "priority:high"),
        vec!["Ancient archived work"]
    );
    assert_eq!(
        archived_titles(&mut env, "\"cold tier\""),
        vec!["Warm archived alpha thing"],
        "full-text search reaches a pruned segment that is not in the checkout"
    );
    assert_eq!(
        archived_titles(&mut env, "data.estimate>4"),
        vec!["Ancient archived work"],
        "a predicate on the JSON blob is evaluated as a residual"
    );
    assert_eq!(
        archived_titles(&mut env, "completed<2025-06-01"),
        vec!["Ancient archived work"]
    );

    // The mixed disjunction: the left branch is pushed into SQL, the right is
    // a residual search, and both sides must survive.
    let mut both = archived_titles(&mut env, "+@work/backend or \"cold tier\"");
    both.sort();
    assert_eq!(
        both,
        vec!["Warm archived alpha thing", "Warm cancelled"],
        "a disjunction mixing a pushed and a residual branch must return both"
    );

    assert_eq!(
        archived_titles(&mut env, "not +@work"),
        vec!["Warm archived alpha thing"]
    );

    // An empty query still lists the whole tier.
    assert_eq!(archived_titles(&mut env, "").len(), 3);
}

#[test]
fn the_archived_tier_refuses_what_it_cannot_answer() {
    let mut env = common::setup();
    corpus(&mut env);

    // `parent:`, `context:` and `user:` are lifted out of the expression
    // before it reaches the store, so they need their own refusal — otherwise
    // the scope is silently dropped and the whole archive comes back.
    for (query, expected) in [
        ("is:blocked", "other tasks"),
        ("created>2026-01-01", "git history"),
        ("parent:alpha", "scopes the whole view"),
        ("context:@work", "scopes the whole view"),
        ("user:alice", "scopes the whole view"),
    ] {
        let err = next::mcp::tools::tasks::list_tasks(
            &serde_json::json!({ "archived": true, "filter_tokens": [query] }),
            &mut env.ctx.repo,
        )
        .expect_err(&format!("{query} should be refused"))
        .to_string();
        assert!(err.contains(expected), "{query}: {err}");
    }
}

#[test]
fn the_archived_tier_paginates_a_residual_query_correctly() {
    // The trap: filtering after LIMIT gives short pages and a total that counts
    // candidates rather than matches.
    let mut env = common::setup();
    corpus(&mut env);

    let result = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({
            "archived": true,
            "filter_tokens": ["archived"],   // a search: residual, not pushed
            "page": 1,
            "page_size": 1
        }),
        &mut env.ctx.repo,
    )
    .unwrap();
    assert_eq!(
        result["total"], 2,
        "two archived tasks mention the word; total must count them, not the tier"
    );
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
}

#[test]
fn atoms_that_need_other_tasks_are_refused_not_silently_empty() {
    // A tier query sees one page of one tier, so these have no answer here.
    // Returning nothing would read as "no matches" instead of "cannot ask".
    use next::core::domain::filter_expr::validate_for_store;

    for (query, expected) in [
        ("is:blocked", "other tasks"),
        ("is:project", "other tasks"),
        ("parent:alpha", "other tasks"),
        ("created>2026-01-01", "git history"),
        ("updated<today", "git history"),
        ("has:created", "git history"),
    ] {
        let err = validate_for_store(&parse(query).unwrap())
            .expect_err(&format!("{query} should be refused"))
            .to_string();
        assert!(err.contains(expected), "{query}: {err}");
    }

    // Everything the archived tier does support stays accepted.
    for query in EXPRESSIONS {
        validate_for_store(&parse(query).unwrap())
            .unwrap_or_else(|e| panic!("{query} should be accepted: {e}"));
    }
}
