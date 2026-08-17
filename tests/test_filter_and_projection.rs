//! What the filter language and field projection promise, across commands.
//!
//! These are the contracts that are not the property of any one command:
//! how `--all` interacts with a `user:` term, where a flag may sit relative to
//! the filter, what a refusal is obliged to tell you, and which fields a
//! projection may name. Each is asserted end-to-end — through clap and
//! `run_with_writer` — because the flag surface and the printed message *are*
//! the contract. A test that built the `Args` struct by hand would not notice
//! a flag that had stopped existing; the per-command test files do that where
//! the behaviour really is local to one command, and this file deliberately
//! does not.
//!
//! `score_breakdown_is_projectable_and_dropped_unless_named` needs the MCP
//! surface, which is behind a non-default feature: run the file with
//! `cargo test --all-features --test test_filter_and_projection`.

mod common;

use std::collections::BTreeSet;

use chrono::Local;
use clap::Parser as _;

use next::cli::commands::{list, show, tree};
use next::cli::{Cli, Command};
use next::core::archiver::run_archive_pass;
use next::core::domain::filter::{self, FilterSet};
use next::core::domain::state::GlobalState;
use next::core::domain::task::Task;
use next::core::projection::Projection;
use next::core::FilterArgs;

// ── Harness ─────────────────────────────────────────────────────────────────

fn today() -> chrono::NaiveDate {
    Local::now().date_naive()
}

/// Parses a full command line the way the binary does.
fn command(argv: &[&str]) -> Result<Command, clap::Error> {
    let mut full = vec!["next"];
    full.extend_from_slice(argv);
    Ok(Cli::try_parse_from(full)?
        .command
        .expect("the argv under test names a subcommand"))
}

fn list_args(argv: &[&str]) -> list::Args {
    let mut full = vec!["list"];
    full.extend_from_slice(argv);
    match command(&full).unwrap_or_else(|e| panic!("next list {argv:?}: {e}")) {
        Command::List(args) => args,
        other => panic!("expected `list`, got {other:?}"),
    }
}

fn try_tree_args(argv: &[&str]) -> Result<tree::Args, clap::Error> {
    let mut full = vec!["tree"];
    full.extend_from_slice(argv);
    match command(&full)? {
        Command::Tree(args) => Ok(args),
        other => panic!("expected `tree`, got {other:?}"),
    }
}

fn show_args(argv: &[&str]) -> show::Args {
    let mut full = vec!["show"];
    full.extend_from_slice(argv);
    match command(&full).unwrap_or_else(|e| panic!("next show {argv:?}: {e}")) {
        Command::Show(args) => args,
        other => panic!("expected `show`, got {other:?}"),
    }
}

/// Runs `next list …` and returns what it wrote to its writer.
fn run_list(env: &common::TestEnv, argv: &[&str]) -> anyhow::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    list::run_with_writer(list_args(argv), &env.ctx, &mut buf)?;
    Ok(String::from_utf8(buf).expect("utf-8 output"))
}

/// Runs `next tree …` and returns what it wrote to its writer.
fn run_tree(env: &common::TestEnv, args: tree::Args) -> anyhow::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    tree::run_with_writer(args, &env.ctx, &mut buf)?;
    Ok(String::from_utf8(buf).expect("utf-8 output"))
}

/// Saves a task and commits it, so the archiver and the git-date lookups have
/// history to read.
fn save(env: &mut common::TestEnv, task: &Task) {
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

/// The error message a query produces, as the CLI would report it.
fn query_error(query: &str) -> String {
    FilterArgs::parse_query(query)
        .err()
        .unwrap_or_else(|| panic!("{query:?} should be refused"))
        .to_string()
}

/// Three tasks completed long ago, archived — the tier `--archived` reads.
fn archived_env() -> common::TestEnv {
    let mut env = common::setup();
    let root = env.ctx.repo.repo_root.clone();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/archive.toml"),
        "archive_after_days = 30\n",
    )
    .unwrap();

    for (i, title) in ["Archived one", "Archived two", "Archived three"]
        .iter()
        .enumerate()
    {
        let mut task = Task::new(*title);
        task.data
            .insert("estimate".into(), serde_json::json!(i as i64 + 1));
        task.mark_done(today() - chrono::Duration::days(200));
        save(&mut env, &task);
    }
    let outcome = run_archive_pass(&mut env.ctx.repo, today()).unwrap();
    assert_eq!(
        outcome.archived, 3,
        "the corpus must reach the archive tier"
    );
    env
}

/// `--all` widens the *implicit gate*; it does not delete a `user:` term the
/// user typed. `apply` used to force `active_users` to `&[]` whenever
/// `disable_implicit` was set, which silently dropped the scope — so
/// `--all user:bob` listed alice's finished work too.
#[test]
fn all_keeps_a_user_term_the_caller_typed() {
    let mut alice = Task::new("Alice's task");
    alice.assignee = Some("alice".into());
    let mut bob = Task::new("Bob's task");
    bob.assignee = Some("bob".into());
    let tasks = vec![alice, bob, Task::new("Unassigned task")];

    // The stored filter says alice; the three cases below differ only in the
    // override, so any difference is the override's doing.
    let state = GlobalState {
        active_users: vec!["alice".to_owned()],
        ..Default::default()
    };
    let titles = |user_override: Option<Vec<String>>| -> Vec<String> {
        let set = FilterSet {
            disable_implicit: true,
            user_override,
            ..Default::default()
        };
        let mut out: Vec<String> = filter::apply(
            tasks.clone(),
            &set,
            &state,
            today(),
            &std::collections::HashMap::new(),
        )
        .into_iter()
        .map(|t| t.title)
        .collect();
        out.sort();
        out
    };

    // 1. An explicit `user:bob` survives `--all`. Unassigned tasks are shared
    //    and stay visible, as they do under every user filter.
    assert_eq!(
        titles(Some(vec!["bob".to_owned()])),
        vec!["Bob's task".to_owned(), "Unassigned task".to_owned()],
        "`--all user:bob` must still be scoped to bob"
    );

    // 2. `--all-users` reaches here as an empty override and means exactly
    //    what it says: no user filter at all.
    assert_eq!(
        titles(Some(vec![])).len(),
        3,
        "--all-users applies no user filter"
    );

    // 3. With no override, the *stored* active users are still bypassed by
    //    `--all` — that part of today's behaviour is deliberate and stays.
    assert_eq!(
        titles(None).len(),
        3,
        "--all still bypasses the stored active_users"
    );
}

/// A flag typed after the filter expression is swallowed by
/// `trailing_var_arg`. Rejecting it as "unrecognised" contradicts `--help`,
/// which lists it; the message has to say the flag must come first.
#[test]
fn a_flag_after_the_filter_says_where_it_belongs() {
    let env = common::setup();

    for argv in [
        vec!["+work", "--fields"],
        vec!["+work", "--fields", "id,title"],
        vec!["+work", "--count"],
        vec!["+work", "--json"],
        vec!["+work", "-n"],
        vec!["+work", "-n", "5"],
    ] {
        // clap hands the flag through as a filter token — that much is by
        // design, and is why the check below has to exist.
        let args = list_args(&argv);
        assert!(
            args.tokens.iter().any(|t| t.starts_with('-')),
            "{argv:?}: clap should have captured the flag as a token"
        );

        let message = run_list(&env, &argv)
            .err()
            .unwrap_or_else(|| panic!("{argv:?} must be refused"))
            .to_string();
        let lower = message.to_lowercase();
        assert!(
            lower.contains("before"),
            "{argv:?}: the message must say the flag comes BEFORE the filter \
             expression, got: {message}"
        );
        assert!(
            !lower.contains("unrecognised flag") && !lower.contains("unrecognized flag"),
            "{argv:?}: `--help` lists this flag, so calling it unrecognised is \
             a contradiction: {message}"
        );
    }

    // A tag exclusion is not a flag and must still work, wherever it sits.
    for argv in [
        vec!["-bug"],
        vec!["-#printer"],
        vec!["-@work"],
        vec!["+work", "-bug"],
        vec!["+work", "-#printer"],
        vec!["+work", "-@work"],
    ] {
        run_list(&env, &argv).unwrap_or_else(|e| panic!("{argv:?} is a tag exclusion: {e}"));
    }
}

/// The "you meant a tag" block only makes sense for an UNSCOPED bare word, and
/// the `+X` it suggests has to be a tag that parses. `title:foo` is not a
/// mistyped tag, and `+arch*` is not a tag at all.
#[test]
fn the_search_hint_is_scoped_and_only_suggests_real_tags() {
    let env = common::setup();
    let explain = |query: &str| run_list(&env, &["--explain", query]).unwrap();

    // A scoped search was deliberate: the user named the field.
    let out = explain("title:foo");
    assert!(
        !out.contains("Read as SEARCHES"),
        "a scoped search is not the bare-token mistake: {out}"
    );

    // An unscoped bare word that IS a valid tag gets the block and the
    // suggestion — the case the block was written for.
    let out = explain("bug");
    assert!(out.contains("Read as SEARCHES"), "{out}");
    assert!(out.contains("write +bug"), "{out}");

    // An unscoped term that is NOT a valid tag still gets the block (it did
    // search, and saying so is useful) but no suggestion, because there is
    // nothing to suggest.
    for query in ["\"cold tier\"", "arch*"] {
        let out = explain(query);
        assert!(
            out.contains("Read as SEARCHES"),
            "{query}: still a search: {out}"
        );
        assert!(
            !out.contains("write +"),
            "{query}: there is no tag of that name to suggest: {out}"
        );
    }

    // Whatever is suggested must be a query the user can actually run.
    for query in ["bug", "café", "wifi-router", "\"cold tier\"", "arch*"] {
        let out = explain(query);
        let Some(rest) = out.split("write +").nth(1) else {
            continue;
        };
        let suggested: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '.')
            .collect();
        next::core::domain::filter_expr::parse(&format!("+{suggested}"))
            .unwrap_or_else(|e| panic!("{query}: suggested `+{suggested}` does not parse: {e}"));
        next::core::domain::tag::validate_tag(&suggested)
            .unwrap_or_else(|e| panic!("{query}: suggested `+{suggested}` is not a tag: {e}"));
    }
}

/// The archived `--explain` reported the matched count twice, once labelled
/// "candidates loaded" — a number it cannot know, because SQL paginated. And
/// it offered `--all`, which `--archived` refuses to be combined with.
#[test]
fn the_archived_explain_does_not_invent_a_candidate_count() {
    let env = archived_env();

    for query in ["data.estimate>1", "data.estimate>99"] {
        let out = run_list(&env, &["--archived", "--explain", query]).unwrap();
        assert!(
            out.contains("not counted"),
            "{query}: an inexact pushdown cannot know how many rows it read, \
             and must say so rather than repeat the match count: {out}"
        );
        // `--archived` conflicts with `--all` at the clap level, so suggesting
        // it is advice that cannot be taken.
        assert!(
            !out.contains("implicit gate"),
            "{query}: the archived tier does not apply the implicit gate: {out}"
        );
        assert!(
            !out.contains("try --all"),
            "{query}: --all conflicts with --archived: {out}"
        );
    }

    // The in-memory path is unchanged: it knows its candidate count and the
    // gate really is why an empty result is empty.
    let mut active = common::setup();
    save(&mut active, &Task::new("An open task"));
    let out = run_list(&active, &["--explain", "+nosuchtag"]).unwrap();
    assert!(out.contains("candidates loaded: 1"), "{out}");
    assert!(out.contains("implicit gate"), "{out}");
}

/// `parent:a,b` and `parent:a parent:b` are different mistakes and deserve
/// different messages: one is a set where a single slug belongs, the other is
/// a repeated scope.
#[test]
fn a_parent_set_reads_differently_from_a_repeated_parent() {
    let set = query_error("parent:a,b");
    let repeated = query_error("parent:a parent:b");

    assert!(
        repeated.contains("only be given once"),
        "the repeat message is unchanged: {repeated}"
    );
    assert_ne!(
        set, repeated,
        "a set is not a repeat, and the message must not pretend otherwise"
    );
    assert!(
        set.contains("one slug"),
        "`parent:a,b` must say it takes one slug: {set}"
    );
}

/// A URL or an unknown `field:value` is refused, correctly — but the way to
/// search for it literally (quote it) is the missing half of the message.
#[test]
fn an_unknown_field_says_how_to_search_for_it_literally() {
    for query in ["https://example.com/x", "foo:bar"] {
        let message = query_error(query);
        assert!(message.contains("unknown field"), "{query}: {message}");
        assert!(
            message.to_lowercase().contains("quote"),
            "{query}: the message must offer quoting as the way to search for \
             the token literally: {message}"
        );
    }

    // And the advice must work: the quoted form is a legal query.
    for query in ["\"https://example.com/x\"", "\"foo:bar\""] {
        FilterArgs::parse_query(query).unwrap_or_else(|e| panic!("{query}: {e}"));
    }
}

/// `--fields` with table output did nothing at all. Silently ignoring a flag
/// the user typed is worse than refusing it.
#[test]
fn fields_without_json_is_a_usage_error() {
    let mut env = common::setup();
    let mut task = Task::new("A task to show");
    task.slug = Some("showme".into());
    save(&mut env, &task);

    let err = run_list(&env, &["--fields", "id,title"])
        .expect_err("`list --fields` without --json must be refused")
        .to_string();
    assert!(
        err.contains("--json"),
        "the message must name the fix: {err}"
    );
    run_list(&env, &["--json", "--fields", "id,title"]).expect("--json --fields is the usage");

    let err = show::run(show_args(&["showme", "--fields", "id,title"]), &env.ctx)
        .expect_err("`show --fields` without --json must be refused")
        .to_string();
    assert!(err.contains("--json"), "{err}");
    show::run(
        show_args(&["showme", "--json", "--fields", "id,title"]),
        &env.ctx,
    )
    .expect("--json --fields is the usage");
}

/// `--count` prints a number and no task, so there is nothing for `--fields`
/// to project — the same silent no-op as `--fields` without `--json`, reached
/// by the one route that guard does not cover. `--json` makes it worse, not
/// better: it satisfies the JSON check and still prints a bare count.
#[test]
fn fields_with_count_is_a_usage_error() {
    let mut env = common::setup();
    save(&mut env, &Task::new("A task"));

    for argv in [
        vec!["--count", "--fields", "id"],
        vec!["--json", "--count", "--fields", "id"],
    ] {
        let err = run_list(&env, &argv)
            .expect_err("`--count` with `--fields` must be refused, not silently counted")
            .to_string();
        assert!(
            err.contains("--count") && err.contains("--fields"),
            "the message must name both flags: {err}"
        );
    }

    // `tree` reaches the count by a different path and needs its own guard.
    for argv in [
        vec!["--count", "--fields", "id"],
        vec!["--json", "--count", "--fields", "id"],
    ] {
        let args = try_tree_args(&argv).unwrap_or_else(|e| panic!("next tree {argv:?}: {e}"));
        let err = run_tree(&env, args)
            .expect_err("`tree --count --fields` must be refused too")
            .to_string();
        assert!(err.contains("--count") && err.contains("--fields"), "{err}");
    }

    // Each flag on its own is still fine.
    run_list(&env, &["--count"]).expect("--count alone");
    run_list(&env, &["--json", "--fields", "id"]).expect("--fields with --json alone");
}

/// `--explain` answers a question about the query, in prose, for a person.
/// There is deliberately no JSON form of it, so `--json`, `--fields` and
/// `--count` have nothing to act on — and `--explain` used to win over all
/// three in silence.
#[test]
fn explain_refuses_the_output_flags_it_would_ignore() {
    let mut env = common::setup();
    save(&mut env, &Task::new("A task"));

    for argv in [
        vec!["--explain", "--json"],
        vec!["--explain", "--format", "json"],
        vec!["--explain", "--json", "--fields", "id"],
        vec!["--explain", "--count"],
    ] {
        let err = run_list(&env, &argv)
            .expect_err("`--explain` with an output flag must be refused")
            .to_string();
        assert!(
            err.contains("--explain"),
            "the message must name the flag that won: {err}"
        );
    }

    // The explanation itself is unaffected, on both tiers.
    let out = run_list(&env, &["--explain", "+@work"]).expect("--explain alone");
    assert!(out.contains("Parsed:"), "{out}");
    run_list(&env, &["--archived", "--explain", "+@work"]).expect("--explain on the archive");
}

/// `created` and `updated` are filterable but are not fields of a task — they
/// come from git. "unknown field" sends the reader looking for a typo.
#[test]
fn the_git_derived_fields_explain_themselves() {
    for name in ["created", "updated"] {
        let message = Projection::parse(&[name.to_owned()])
            .err()
            .unwrap_or_else(|| panic!("{name} is not projectable"))
            .to_string();
        let lower = message.to_lowercase();
        assert!(
            lower.contains("git"),
            "{name}: the message must say the value is git-derived: {message}"
        );
        assert!(
            lower.contains("filter"),
            "{name}: …and that it can still be filtered on: {message}"
        );
    }

    // A genuine typo keeps the generic message.
    let message = Projection::parse(&["nosuchfield".to_owned()])
        .unwrap_err()
        .to_string();
    assert!(message.contains("unknown field"), "{message}");
}

/// `score_breakdown` is a big object that a projecting caller never asked for.
/// It becomes projectable, and — like `score` in `list` — is dropped unless
/// named. With no projection, nothing changes.
#[cfg(feature = "mcp")]
#[test]
fn score_breakdown_is_projectable_and_dropped_unless_named() {
    let mut env = common::setup();
    let mut task = Task::new("A task to fetch");
    task.slug = Some("fetchme".into());
    save(&mut env, &task);

    let get = |env: &mut common::TestEnv, params: serde_json::Value| -> serde_json::Value {
        next::mcp::tools::tasks::get_task(&params, &mut env.ctx.repo).unwrap()
    };

    // No projection: the response is what it always was.
    let full = get(&mut env, serde_json::json!({ "id": "fetchme" }));
    assert!(full.get("score").is_some(), "{full}");
    assert!(full.get("score_breakdown").is_some(), "{full}");

    // A projection that does not name them drops both.
    let lean = get(
        &mut env,
        serde_json::json!({ "id": "fetchme", "fields": ["id", "title"] }),
    );
    assert!(
        lean.get("score_breakdown").is_none(),
        "a caller asking for id,title is not asking for the breakdown: {lean}"
    );
    assert!(
        lean.get("score").is_none(),
        "…and `list` already drops `score` on the same rule: {lean}"
    );

    // Naming it keeps it, and only it.
    let asked = get(
        &mut env,
        serde_json::json!({ "id": "fetchme", "fields": ["id", "score_breakdown"] }),
    );
    assert!(
        asked.get("score_breakdown").is_some(),
        "score_breakdown must be a projectable name: {asked}"
    );
    assert!(asked.get("score").is_none(), "{asked}");
}

/// Projection is a property of the JSON output, not of one command: `tree` and
/// `next` emit tasks as JSON too, and pay the same payload cost.
#[test]
fn tree_and_next_accept_fields_on_the_command_line() {
    let mut env = common::setup();
    save(&mut env, &Task::new("A task in the tree"));

    // `tree --count` behaves as `list --count` does — answered before the
    // format, counting matches rather than the ancestors kept for shape — and
    // test_tree.rs asserts that. What only a parsed command line can show is
    // that the flags reached the command at all.
    try_tree_args(&["--json", "--count"]).expect("`tree` must accept --count beside --json");

    let args =
        try_tree_args(&["--json", "--fields", "id,title"]).expect("`tree` must accept --fields");
    let out = run_tree(&env, args).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let items = parsed.as_array().expect("tree --json is an array");
    assert!(!items.is_empty(), "the corpus must produce a row");
    for item in items {
        let keys: BTreeSet<&str> = item
            .as_object()
            .expect("a task object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from(["id", "title"]),
            "projection keeps exactly what was asked for: {item}"
        );
    }

    // `next` writes its JSON to stdout rather than to an injectable writer, so
    // what is pinned here is the surface: the flag exists and parses.
    command(&["next", "--json", "--fields", "id,title"]).expect("`next` must accept --fields");
}

/// The active list honours `list_limit`; the archived one ignored it and used
/// the built-in default, so the same config gave two different page sizes.
#[test]
fn the_archived_page_size_honours_list_limit() {
    let mut env = archived_env();
    env.ctx.config.list_limit = Some(1);

    let out = run_list(&env, &["--archived", "--json"]).unwrap();
    let page: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(page["total"], 3, "the config caps the page, not the query");
    assert_eq!(
        page["items"].as_array().unwrap().len(),
        1,
        "`list_limit` must size the archived page as it sizes the active one: {out}"
    );

    // An explicit flag still wins over the config.
    let out = run_list(&env, &["--archived", "--json", "--page-size", "2"]).unwrap();
    let page: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
}

/// A query whose status predicate the implicit gate contradicts returns
/// nothing, correctly, and says nothing about why. Which tasks match must NOT
/// change — only the hint is new.
///
/// The hint is asserted on `run_with_writer`'s writer, which is what that
/// entry point exists for.
#[test]
fn a_contradicted_status_query_hints_at_all() {
    let mut env = common::setup();
    save(&mut env, &Task::new("An open task"));

    for argv in [
        vec!["status:done"],
        vec!["is:closed"],
        vec!["--closed", "status:open"],
    ] {
        let out = run_list(&env, &argv).unwrap();
        assert!(
            out.contains("--all"),
            "{argv:?}: an empty result caused by the implicit gate must name \
             --all, got: {out:?}"
        );
    }

    // A query that legitimately matched nothing gets no such hint — `--all`
    // would not have helped, and a hint that is usually wrong is noise.
    let out = run_list(&env, &["+nosuchtag"]).unwrap();
    assert!(
        !out.contains("--all"),
        "nothing about the gate explains an empty tag query: {out:?}"
    );

    // The hint is advice, not a filter change: the counts are what they were.
    assert_eq!(
        run_list(&env, &["--count", "status:done"]).unwrap().trim(),
        "0"
    );
    assert_eq!(run_list(&env, &["--count"]).unwrap().trim(), "1");
    assert_eq!(
        run_list(&env, &["--count", "--all", "status:done"])
            .unwrap()
            .trim(),
        "0",
        "--all really does show nothing here — there are no closed tasks"
    );
}
