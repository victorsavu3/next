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
    // Search — pushed to the FTS index as of S3, and marked exact, so these
    // are the cases proving the index and the scan are one dialect.
    "alpha",
    "\"the cold\"",
    "arch*",
    "title:beta",
    "notes:measured",
    // Case and accent folding: the scan lowercases without stripping
    // diacritics, so the index is built `remove_diacritics 0` to match.
    "café",
    "CAFÉ",
    "Café",
    "naïve",
    "cafe",
    "resume",
    // CJK — the likeliest place for two tokenizers to part ways.
    "日本語",
    "テスト",
    // Words that are FTS5 operators. Quoting the rebuilt phrase is what keeps
    // these literal instead of syntax (or a MATCH parse error). `and`/`or` are
    // reserved in *our* grammar too, so they arrive already quoted; `near` is
    // FTS5-only and reaches the index as a bare word.
    "\"and\"",
    "\"OR\"",
    "near",
    "NEAR",
    // Characters that are FTS5 syntax. They tokenise away on both sides.
    "\"sym*bol\"",
    "\"12345\"",
    "🎉",
    "\"---\"",
    // Hyphenation and digits split the same way on both sides.
    "wifi",
    "router",
    "\"wifi router\"",
    "12345",
    // Prefix search over the same material.
    "caf*",
    "wifi-rout*",
    "日本*",
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
        snap_leeway: None,
    });
    active.push(gamma);

    // Untagged and unassigned, so `user:` and the tag atoms have a negative case.
    active.push(Task::new("Delta plain"));

    // The text that decides whether the FTS index and the in-memory scan are
    // the same dialect: accents, CJK, emoji, hyphenation, digits, and words
    // that are FTS5 operators. If these two ever disagree, the search atom
    // must stop claiming to be exact.
    let mut unicode = Task::new("Café naïve résumé");
    unicode.description = Some("日本語 テスト and OR not NEAR".into());
    unicode.notes = Some("wifi-router 12345 🎉 sym*bol \"quoted\"".into());
    active.push(unicode);

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
    // Ancient enough to be pruned into the cold tier. `sarcophagus` appears
    // nowhere else, so searching for it proves the pruned segment — which is
    // not in the working tree at all — is indexed like everything else.
    let mut ancient = Task::new("Ancient archived work");
    ancient.notes = Some("Filed inside a sarcophagus.".into());
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
fn sql_and_in_memory_agree_on_every_spelling_of_an_id() {
    // `id:` cannot go in EXPRESSIONS — the values are only known once the
    // corpus exists — but it is exactly the kind of atom that wants the
    // differential treatment: the evaluator compares a dashless string while
    // the column stores a hyphenated one, and the atom is marked exact, so a
    // disagreement would silently drop rows rather than fail.
    let mut env = common::setup();
    let (active, archived) = corpus(&mut env);
    let store = env.ctx.repo.store();

    for (tasks, is_archived) in [(&active, false), (&archived, true)] {
        let subject = tasks.first().expect("corpus is not empty");
        let simple = subject.id.simple().to_string();
        let queries = [
            format!("id:{}", &simple[..8]),
            format!("id:{}", subject.id.hyphenated()),
            format!("id:{simple}"),
            format!("id:{}", simple[..8].to_uppercase()),
            // A prefix that names nothing, and one that names this task among
            // a set.
            "id:ffffffff".to_owned(),
            format!("id:{},ffffffff", &simple[..8]),
            format!("not id:{}", &simple[..8]),
            format!("id:{} and +@work", &simple[..8]),
            "has:id".to_owned(),
            "no:id".to_owned(),
        ];
        for query in &queries {
            assert_paths_agree(store, tasks, query, is_archived);
        }
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
///
/// The MCP surface is behind a non-default feature, so these three tests are
/// gated. Without the gate a plain `cargo test` fails to compile the whole
/// target — the store-level differentials above included.
#[cfg(feature = "mcp")]
fn archived_titles(env: &mut common::TestEnv, query: &str) -> Vec<String> {
    let result = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({ "archived": true, "filter": query }),
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

#[cfg(feature = "mcp")]
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

/// `--all-users` means "apply no user filter", which is what an archive
/// listing already does — so it must be a no-op, not a refusal about a `user:`
/// term nobody typed. It reaches the store as `user_override = Some(vec![])`,
/// which the guard used to treat as a real view term.
#[test]
fn all_users_is_a_no_op_on_the_archived_tier() {
    let mut env = common::setup();
    corpus(&mut env);

    let mut args = next::core::FilterArgs::parse(vec![]).unwrap();
    args.all_users = true;
    let filter_set = args.to_filter_set().unwrap();
    let filter = filter_set
        .to_store_filter(today())
        .expect("--all-users must not be refused");

    let page = env
        .ctx
        .repo
        .store()
        .query_tasks(&TaskQuery {
            archived: true,
            filter: Some(filter),
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    assert_eq!(page.total, 3, "the whole archive, unfiltered by user");

    // A named user is still a real view term and still refused.
    let named = next::core::FilterArgs::parse(vec!["user:alice".into()])
        .unwrap()
        .to_filter_set()
        .unwrap();
    let err = named.to_store_filter(today()).unwrap_err().to_string();
    assert!(err.contains("scopes the whole view"), "{err}");
}

/// The S4 break: `filter_tokens` and `context` are gone, and an agent holding
/// the old schema is told what replaced them rather than left guessing. An
/// ignored `filter_tokens` would read as "no filter" and return the whole
/// list looking perfectly successful, which is the failure worth preventing.
#[cfg(feature = "mcp")]
#[test]
fn the_removed_parameters_name_their_replacement() {
    let mut env = common::setup();
    corpus(&mut env);

    let err = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({ "filter_tokens": ["+@work"] }),
        &mut env.ctx.repo,
    )
    .expect_err("filter_tokens must be refused")
    .to_string();
    assert!(err.contains("replaced by filter"), "{err}");
    assert!(err.contains("one string"), "{err}");

    let err = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({ "context": ["@work"] }),
        &mut env.ctx.repo,
    )
    .expect_err("context must be refused")
    .to_string();
    assert!(err.contains("put the tag in filter"), "{err}");

    // The forecast tool took both too, so it refuses both as well.
    let err = next::mcp::tools::view::get_forecast(
        &serde_json::json!({ "filter_tokens": ["+@work"] }),
        &mut env.ctx.repo,
    )
    .expect_err("get_forecast must refuse it too")
    .to_string();
    assert!(err.contains("replaced by filter"), "{err}");
}

/// Field projection through the MCP tools, which is where the payload cost
/// actually bites — an agent pays for every field of every row on every call.
#[cfg(feature = "mcp")]
#[test]
fn mcp_projection_trims_the_payload() {
    let mut env = common::setup();
    corpus(&mut env);

    let full =
        next::mcp::tools::tasks::list_tasks(&serde_json::json!({}), &mut env.ctx.repo).unwrap();
    let lean = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({ "fields": ["id", "title"] }),
        &mut env.ctx.repo,
    )
    .unwrap();

    assert_eq!(
        full["total"], lean["total"],
        "projection must not change which tasks match"
    );
    let task = lean["items"][0]["task"].as_object().unwrap();
    assert_eq!(task.len(), 2, "{task:?}");
    assert!(task.contains_key("id") && task.contains_key("title"));

    // The point of the feature, measured rather than asserted in the abstract.
    let full_len = serde_json::to_string(&full).unwrap().len();
    let lean_len = serde_json::to_string(&lean).unwrap().len();
    assert!(
        lean_len * 2 < full_len,
        "the lean response should be far smaller: {lean_len} vs {full_len}"
    );

    // `get_task` projects the task and its children alike.
    let one = next::mcp::tools::tasks::get_task(
        &serde_json::json!({ "id": "alpha", "fields": ["id", "title"] }),
        &mut env.ctx.repo,
    )
    .unwrap();
    assert_eq!(one["task"].as_object().unwrap().len(), 2);

    // An unknown name is refused with the alternatives, not ignored.
    let err = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({ "fields": ["nosuchfield"] }),
        &mut env.ctx.repo,
    )
    .expect_err("unknown field must be refused")
    .to_string();
    assert!(err.contains("unknown field"), "{err}");
}

#[cfg(feature = "mcp")]
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
            &serde_json::json!({ "archived": true, "filter": query }),
            &mut env.ctx.repo,
        )
        .expect_err(&format!("{query} should be refused"))
        .to_string();
        assert!(err.contains(expected), "{query}: {err}");
    }
}

#[cfg(feature = "mcp")]
#[test]
fn the_archived_tier_paginates_a_residual_query_correctly() {
    // The trap: filtering after LIMIT gives short pages and a total that counts
    // candidates rather than matches.
    let mut env = common::setup();
    corpus(&mut env);

    let result = next::mcp::tools::tasks::list_tasks(
        &serde_json::json!({
            "archived": true,
            "filter": "archived",
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

/// The payoff of indexing the archive: text in a segment that has been pruned
/// out of the checkout entirely is still searchable. `grep` cannot reach it.
#[test]
fn a_pruned_cold_segment_is_searchable() {
    let mut env = common::setup();
    corpus(&mut env);
    let root = env.ctx.repo.repo_root.clone();

    // The segment really is gone from the working tree.
    assert!(
        !root.join("archive/2025/03-001.toml").exists(),
        "the cold segment should have been pruned out of the checkout"
    );

    let hits = env
        .ctx
        .repo
        .store()
        .query_tasks(&TaskQuery {
            archived: true,
            filter: Some(QueryFilter::new(parse("sarcophagus").unwrap(), today())),
            ..TaskQuery::unpaginated()
        })
        .unwrap();
    assert_eq!(hits.total, 1, "the pruned segment's text must be findable");
    assert_eq!(hits.items[0].title, "Ancient archived work");
}

/// A cache written by an older schema must rebuild itself, virtual table and
/// all — otherwise the first search after an upgrade queries a table that does
/// not exist, or one left empty by the rebuild that skipped it.
#[test]
fn an_older_cache_rebuilds_its_index_on_open() {
    let mut env = common::setup();
    corpus(&mut env);
    let root = env.ctx.repo.repo_root.clone();

    // Re-open at a fresh path so the cache is built from scratch, then confirm
    // search works — the baseline the upgrade has to reach.
    let open = |db: &str| {
        let inner =
            next::core::storage::TomlStore::open(root.clone(), root.join("state.toml")).unwrap();
        let vcs = next::core::storage::GitBackend::open(&root).unwrap();
        let head = next::core::store::VcsBackend::head_hash(&vcs).unwrap();
        next::core::storage::CachedStore::open(inner, root.join(db), &head).unwrap()
    };

    let fresh = open(".next-upgrade.db");
    let search = |store: &dyn Store, term: &str| {
        store
            .query_tasks(&TaskQuery {
                archived: true,
                filter: Some(QueryFilter::new(parse(term).unwrap(), today())),
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total
    };
    assert_eq!(search(&fresh, "sarcophagus"), 1);
    drop(fresh);

    // Now pretend it was written by the previous schema and re-open.
    let conn = rusqlite::Connection::open(root.join(".next-upgrade.db")).unwrap();
    conn.execute(
        "UPDATE meta SET value = '3' WHERE key = 'schema_version'",
        [],
    )
    .unwrap();
    drop(conn);

    let upgraded = open(".next-upgrade.db");
    assert_eq!(
        search(&upgraded, "sarcophagus"),
        1,
        "search must work again after the schema self-heal"
    );
}

/// Guards the guard: the search cases in the corpus must actually match
/// something. A differential where both paths return nothing agrees perfectly
/// and proves nothing — the same trap the old bare-token test fell into.
#[test]
fn the_unicode_search_cases_are_not_vacuous() {
    let mut env = common::setup();
    corpus(&mut env);
    let store = env.ctx.repo.store();

    let matches = |query: &str| -> u64 {
        let expr = parse(query).unwrap_or_else(|e| panic!("{query}: {e}"));
        store
            .query_tasks(&TaskQuery {
                filter: Some(QueryFilter::new(expr, today())),
                ..TaskQuery::unpaginated()
            })
            .unwrap()
            .total
    };

    // Positive controls: each of these must find the unicode task.
    for query in [
        "café",
        "CAFÉ",
        "naïve",
        "日本語",
        "テスト",
        "near",
        "NEAR",
        "wifi",
        "router",
        "\"wifi router\"",
        "12345",
        "caf*",
        "日本*",
        "\"and\"",
        "\"sym*bol\"",
    ] {
        assert!(
            matches(query) > 0,
            "{query} matched nothing — its differential case is vacuous"
        );
    }

    // Negative controls: the accent rule and the word-boundary rule are doing
    // real work, and a term of pure punctuation finds nothing.
    for query in ["cafe", "resume", "🎉", "\"---\"", "zzzznothing"] {
        assert_eq!(matches(query), 0, "{query} should match nothing");
    }
}
