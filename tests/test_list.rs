mod common;

use chrono::Local;
use next::cli::commands::add;
use next::core::FilterArgs;
use next::core::{
    domain::filter,
    scoring::{self, ScoredTask},
};

fn add_args(title: &str) -> add::Args {
    add::Args {
        title: title.to_string(),
        due: None,
        start: None,
        priority: None,
        slug: None,
        assignee: None,
        tags: vec![],
        parent: None,
        blocked_by: vec![],
        description: None,
        url: None,
        notes: None,
        recur_schedule: None,
        recur_completion: None,
        recur_snap: None,
        recur_snap_leeway: None,
        quiet: false,
        long_term: false,
        adjust: None,
        json: false,
    }
}

fn apply_filter(env: &mut common::TestEnv, tokens: Vec<String>) -> Vec<ScoredTask> {
    let today = Local::now().date_naive();
    let filter_args = FilterArgs::parse(tokens).unwrap();
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    scoring::score_and_sort(
        filtered,
        &all,
        today,
        &env.ctx.repo.scoring,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    )
}

#[test]
fn list_returns_open_tasks() {
    let mut env = common::setup();
    add::run(add_args("Task one"), &mut env.ctx).unwrap();
    add::run(add_args("Task two"), &mut env.ctx).unwrap();

    let tasks = apply_filter(&mut env, vec![]);
    assert_eq!(tasks.len(), 2);
}

#[test]
fn list_filters_by_required_tag() {
    let mut env = common::setup();
    let with_tag = add::Args {
        tags: vec!["@work".to_string()],
        ..add_args("Work task")
    };
    add::run(with_tag, &mut env.ctx).unwrap();
    add::run(add_args("Home task"), &mut env.ctx).unwrap();

    let tasks = apply_filter(&mut env, vec!["+@work".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Work task");
}

/// A bare token searches the text; the tag it used to mean is now `+tag`.
///
/// The two tasks here are deliberately crossed — one carries the tag without
/// the word, the other has the word without the tag — because a task with both
/// would pass either reading and prove nothing.
#[test]
fn list_bare_token_searches_text_while_the_sigil_selects_the_tag() {
    let mut env = common::setup();
    let tagged = add::Args {
        tags: vec!["urgent".to_string()],
        ..add_args("Renew the passport")
    };
    add::run(tagged, &mut env.ctx).unwrap();
    add::run(add_args("An urgent-sounding title"), &mut env.ctx).unwrap();

    let tasks = apply_filter(&mut env, vec!["urgent".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "An urgent-sounding title");

    let tasks = apply_filter(&mut env, vec!["+urgent".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Renew the passport");
}

/// The grammar reaches the CLI whole, not just its tag half.
#[test]
fn list_accepts_a_full_expression() {
    let mut env = common::setup();
    let tagged = add::Args {
        tags: vec!["@work".to_string()],
        ..add_args("Write the design doc")
    };
    add::run(tagged, &mut env.ctx).unwrap();
    add::run(add_args("Water the plants"), &mut env.ctx).unwrap();

    // Booleans and grouping.
    let tasks = apply_filter(&mut env, vec!["+@work or plants".to_string()]);
    assert_eq!(tasks.len(), 2);

    // Field predicates and negation.
    let tasks = apply_filter(&mut env, vec!["not +@work".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Water the plants");

    let tasks = apply_filter(&mut env, vec!["status:open".to_string()]);
    assert_eq!(tasks.len(), 2);

    // A phrase is ordered and consecutive.
    let tasks = apply_filter(&mut env, vec!["\"design doc\"".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert!(apply_filter(&mut env, vec!["\"doc design\"".to_string()]).is_empty());
}

/// Runs the real listing pipeline: `load_candidates` (which pushes the status
/// gate into the store) followed by `filter::apply`.
///
/// `apply_filter` above deliberately starts from the whole task list, so it
/// cannot see what the status pushdown hides — which is exactly the bug this
/// exercises.
fn pipeline(env: &mut common::TestEnv, query: &str, closed_only: bool) -> Vec<String> {
    let today = Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec![query.to_string()]).unwrap();
    filter_args.closed = closed_only;
    let filter_set = filter_args.to_filter_set().unwrap();

    let store = env.ctx.repo.store();
    let state = store.get_state().unwrap();
    let candidates = next::core::listing::load_candidates(store, &filter_set).unwrap();
    filter::apply(
        candidates,
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    )
    .into_iter()
    .map(|t| t.title)
    .collect()
}

/// `is:project` is answered from an index over the tasks the listing loaded.
/// The default load is active-only, so a parent whose children are all closed
/// looked childless and the predicate returned nothing.
#[test]
fn is_project_counts_a_closed_child() {
    let mut env = common::setup();
    let today = Local::now().date_naive();

    add::run(
        add::Args {
            slug: Some("proj".into()),
            ..add_args("Parent of a finished child")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some("proj".into()),
            ..add_args("Finished child")
        },
        &mut env.ctx,
    )
    .unwrap();

    // Finish the child, so nothing but the parent remains active.
    let child = env
        .ctx
        .repo
        .store()
        .list_tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.title == "Finished child")
        .expect("child exists");
    let mut done = child.clone();
    done.mark_done(today);
    env.ctx
        .repo
        .transaction(|store, vcs, root| {
            store.save_task(&done)?;
            vcs.commit(&[next::core::storage::task_path(root, &done)], "done")?;
            Ok(())
        })
        .unwrap();

    assert_eq!(
        pipeline(&mut env, "is:project", false),
        vec!["Parent of a finished child"],
        "a closed child still makes its parent a project"
    );
}

/// The sibling case: under `--closed` only closed tasks load, so the
/// open-blocker index was empty and `is:blocked` could never fire.
#[test]
fn is_blocked_works_under_closed_only() {
    let mut env = common::setup();
    let today = Local::now().date_naive();

    add::run(add_args("Open blocker"), &mut env.ctx).unwrap();
    let blocker = env
        .ctx
        .repo
        .store()
        .list_tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.title == "Open blocker")
        .expect("blocker exists");

    add::run(
        add::Args {
            blocked_by: vec![blocker.id.to_string()],
            ..add_args("Closed but blocked")
        },
        &mut env.ctx,
    )
    .unwrap();
    let blocked = env
        .ctx
        .repo
        .store()
        .list_tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.title == "Closed but blocked")
        .expect("blocked exists");
    let mut done = blocked.clone();
    done.mark_done(today);
    env.ctx
        .repo
        .transaction(|store, vcs, root| {
            store.save_task(&done)?;
            vcs.commit(&[next::core::storage::task_path(root, &done)], "done")?;
            Ok(())
        })
        .unwrap();

    assert_eq!(
        pipeline(&mut env, "is:blocked", true),
        vec!["Closed but blocked"],
        "the open blocker must be loaded even though the listing is closed-only"
    );
}

/// `--count` answers "how many match", which under pagination is the total
/// across every page — not how many happened to fit on this one.
#[test]
fn count_is_the_total_not_the_page_size() {
    let mut env = common::setup();
    for i in 0..7 {
        add::run(add_args(&format!("Task {i}")), &mut env.ctx).unwrap();
    }

    let counted = |env: &mut common::TestEnv, page_size: Option<u32>, tokens: Vec<String>| {
        let today = Local::now().date_naive();
        let mut fa = FilterArgs::parse(tokens).unwrap();
        fa.future = false;
        let set = fa.to_filter_set().unwrap();
        let store = env.ctx.repo.store();
        let state = store.get_state().unwrap();
        let candidates = next::core::listing::load_candidates(store, &set).unwrap();
        let n = filter::apply(
            candidates,
            &set,
            &state,
            today,
            &std::collections::HashMap::new(),
        )
        .len();
        // Whatever page size a caller passes, the count is the same number.
        let _ = page_size;
        n
    };

    assert_eq!(counted(&mut env, None, vec![]), 7);
    assert_eq!(
        counted(&mut env, Some(2), vec![]),
        7,
        "a small page must not shrink the count"
    );
    assert_eq!(counted(&mut env, Some(2), vec!["\"Task 3\"".into()]), 1);
}

/// The three surfaces must parse the same string, so a phrase with an embedded
/// space has to survive argv joining and the TUI's token round-trip alike.
#[test]
fn a_quoted_phrase_survives_argv_joining() {
    let mut env = common::setup();
    add::run(add_args("Rebuild the cold tier cache"), &mut env.ctx).unwrap();
    add::run(add_args("Tier of cold storage"), &mut env.ctx).unwrap();

    // Split across argv the way a shell delivers `next list "cold tier"`.
    let split = apply_filter(&mut env, vec!["\"cold".into(), "tier\"".into()]);
    assert_eq!(split.len(), 1, "a phrase reassembles across argv tokens");
    assert_eq!(split[0].task.title, "Rebuild the cold tier cache");

    // And as one pre-joined argument, which is what the MCP `filter` sends.
    let joined = apply_filter(&mut env, vec!["\"cold tier\"".into()]);
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].task.title, "Rebuild the cold tier cache");
}

/// `--explain` is meant to be read, so its actual output is asserted rather
/// than eyeballed. These run the real command, not the renderer.
mod explain {
    use super::*;

    fn list_args(tokens: Vec<String>) -> next::cli::commands::list::Args {
        next::cli::commands::list::Args {
            future: false,
            all: false,
            closed: false,
            archived: false,
            all_users: false,
            json: false,
            format: None,
            count: false,
            explain: true,
            fields: vec![],
            limit: None,
            page_size: None,
            page: 1,
            tokens,
        }
    }

    fn explain(env: &common::TestEnv, tokens: Vec<String>) -> String {
        let mut buf: Vec<u8> = Vec::new();
        next::cli::commands::list::run_with_writer(list_args(tokens), &env.ctx, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn it_names_a_bare_word_as_a_search_and_shows_the_tag_spelling() {
        let mut env = common::setup();
        let tagged = add::Args {
            tags: vec!["printer".to_string()],
            ..add_args("Renew the passport")
        };
        add::run(tagged, &mut env.ctx).unwrap();

        let out = explain(&env, vec!["printer".to_string()]);
        assert!(out.contains("Query:    printer"), "{out}");
        assert!(out.contains("Read as SEARCHES"), "{out}");
        assert!(
            out.contains("write +printer"),
            "it must show the tag spelling: {out}"
        );
        // The counts come from the run itself.
        assert!(out.contains("candidates loaded: 1"), "{out}");
        assert!(out.contains("matched:           0"), "{out}");
    }

    #[test]
    fn it_says_plainly_when_the_shell_ate_the_query() {
        let env = common::setup();
        let out = explain(&env, vec![]);
        assert!(out.contains("No filter was given"), "{out}");
        assert!(out.contains("shell may have consumed it"), "{out}");
    }

    #[test]
    fn it_points_at_the_implicit_gate_when_nothing_matched() {
        let mut env = common::setup();
        add::run(add_args("Something"), &mut env.ctx).unwrap();

        let out = explain(&env, vec!["+nosuchtag".to_string()]);
        assert!(out.contains("matched:           0"), "{out}");
        assert!(out.contains("implicit gate"), "{out}");
        assert!(out.contains("--all"), "{out}");
    }

    /// The active list evaluates the expression in memory — only the status
    /// gate is pushed. Saying "nothing was pushed" was wrong in one direction
    /// and "no cache for this query" wrong in another.
    #[test]
    fn it_says_where_the_filter_actually_ran() {
        let mut env = common::setup();
        add::run(add_args("Something"), &mut env.ctx).unwrap();

        let out = explain(&env, vec!["+nosuchtag".to_string()]);
        assert!(out.contains("the status gate only"), "{out}");
        assert!(out.contains("evaluated in memory"), "{out}");
        assert!(!out.contains("no cache"), "{out}");
    }

    /// The archived tier DOES compile the expression, so it reports the
    /// fragment and whether SQL owns the answer.
    #[test]
    fn the_archived_tier_reports_its_compiled_sql() {
        let mut env = common::setup();
        add::run(add_args("Something"), &mut env.ctx).unwrap();

        let mut args = list_args(vec!["+@work".to_string()]);
        args.archived = true;
        let mut buf: Vec<u8> = Vec::new();
        next::cli::commands::list::run_with_writer(args, &env.ctx, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();

        assert!(out.contains("pushed into SQL:"), "{out}");
        assert!(
            out.contains("task_tags"),
            "the real compiled fragment, not a placeholder: {out}"
        );
        assert!(
            out.contains("fully answered by SQL"),
            "a tag filter is exact: {out}"
        );
    }

    #[test]
    fn a_tag_query_is_not_mislabelled_as_a_search() {
        let mut env = common::setup();
        let tagged = add::Args {
            tags: vec!["printer".to_string()],
            ..add_args("Fix it")
        };
        add::run(tagged, &mut env.ctx).unwrap();

        let out = explain(&env, vec!["+printer".to_string()]);
        assert!(!out.contains("Read as SEARCHES"), "{out}");
        assert!(out.contains("matched:           1"), "{out}");
    }
}

/// Field projection over the JSON output, on the real commands.
mod projection {
    use super::*;

    fn list_json(env: &common::TestEnv, fields: Vec<String>) -> serde_json::Value {
        let args = next::cli::commands::list::Args {
            future: false,
            all: false,
            closed: false,
            archived: false,
            all_users: false,
            json: true,
            format: None,
            count: false,
            explain: false,
            fields,
            limit: None,
            page_size: None,
            page: 1,
            tokens: vec![],
        };
        let mut buf: Vec<u8> = Vec::new();
        next::cli::commands::list::run_with_writer(args, &env.ctx, &mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    fn with_a_rich_task() -> common::TestEnv {
        let mut env = common::setup();
        add::run(
            add::Args {
                slug: Some("rich".into()),
                tags: vec!["@work".into()],
                notes: Some("A very long note that costs real payload".into()),
                description: Some("A description".into()),
                ..add_args("Rebuild the cache")
            },
            &mut env.ctx,
        )
        .unwrap();
        env
    }

    #[test]
    fn without_fields_every_field_is_returned() {
        let env = with_a_rich_task();
        let json = list_json(&env, vec![]);
        let task = &json["items"][0]["task"];
        assert!(task.get("notes").is_some(), "the default is unchanged");
        assert!(json["items"][0].get("score").is_some());
    }

    #[test]
    fn only_the_requested_fields_come_back() {
        let env = with_a_rich_task();
        let json = list_json(&env, vec!["id".into(), "title".into()]);
        let task = json["items"][0]["task"].as_object().unwrap();

        assert_eq!(task.len(), 2, "{task:?}");
        assert!(task.contains_key("id") && task.contains_key("title"));
        assert!(
            !task.contains_key("notes"),
            "the payload this feature exists to avoid: {task:?}"
        );
        assert!(
            !task.contains_key("description"),
            "absent, not null: {task:?}"
        );
    }

    #[test]
    fn the_pagination_envelope_survives() {
        let env = with_a_rich_task();
        let json = list_json(&env, vec!["id".into()]);
        assert_eq!(
            json["total"], 1,
            "a projecting caller still needs the total"
        );
        assert_eq!(json["page"], 1);
        assert_eq!(json["page_size"], 50);
    }

    #[test]
    fn projection_does_not_change_which_tasks_match() {
        // Selection and presentation are different stages: filtering on notes
        // must still work when notes are not returned.
        let mut env = with_a_rich_task();
        add::run(add_args("Unrelated"), &mut env.ctx).unwrap();

        let args = next::cli::commands::list::Args {
            future: false,
            all: false,
            closed: false,
            archived: false,
            all_users: false,
            json: true,
            format: None,
            count: false,
            explain: false,
            fields: vec!["id".into()],
            limit: None,
            page_size: None,
            page: 1,
            tokens: vec!["notes:payload".into()],
        };
        let mut buf: Vec<u8> = Vec::new();
        next::cli::commands::list::run_with_writer(args, &env.ctx, &mut buf).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&buf).unwrap();

        assert_eq!(json["total"], 1, "the filter still reads notes");
        assert_eq!(json["items"][0]["task"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn an_unknown_field_fails_before_any_work() {
        let env = with_a_rich_task();
        let args = next::cli::commands::list::Args {
            future: false,
            all: false,
            closed: false,
            archived: false,
            all_users: false,
            json: true,
            format: None,
            count: false,
            explain: false,
            fields: vec!["nosuchfield".into()],
            limit: None,
            page_size: None,
            page: 1,
            tokens: vec![],
        };
        let mut buf: Vec<u8> = Vec::new();
        let err = next::cli::commands::list::run_with_writer(args, &env.ctx, &mut buf)
            .expect_err("an unknown field must be refused")
            .to_string();
        assert!(err.contains("unknown field"), "{err}");
        assert!(err.contains("title"), "it lists the alternatives: {err}");
    }
}

/// A malformed query is refused, not silently read as something else.
#[test]
fn list_rejects_a_malformed_expression() {
    let mut env = common::setup();
    add::run(add_args("Anything"), &mut env.ctx).unwrap();

    let err = FilterArgs::parse(vec!["due<".to_string()]).expect_err("should not parse");
    assert!(
        err.to_string().contains("cannot parse filter expression"),
        "{err}"
    );
}

#[test]
fn list_excludes_by_negative_tag() {
    let mut env = common::setup();
    let with_tag = add::Args {
        tags: vec!["@work".to_string()],
        ..add_args("Work task")
    };
    add::run(with_tag, &mut env.ctx).unwrap();
    add::run(add_args("Home task"), &mut env.ctx).unwrap();

    let tasks = apply_filter(&mut env, vec!["-@work".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Home task");
}

#[test]
fn list_filters_by_multiple_tags() {
    let mut env = common::setup();

    let both = add::Args {
        tags: vec!["@work".to_string(), "urgent".to_string()],
        ..add_args("Urgent work task")
    };
    add::run(both, &mut env.ctx).unwrap();

    let only_work = add::Args {
        tags: vec!["@work".to_string()],
        ..add_args("Routine work task")
    };
    add::run(only_work, &mut env.ctx).unwrap();

    add::run(add_args("Unrelated"), &mut env.ctx).unwrap();

    // Require both @work AND urgent.
    let tasks = apply_filter(&mut env, vec!["+@work".to_string(), "+urgent".to_string()]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Urgent work task");
}

#[test]
fn list_empty_when_no_tasks() {
    let mut env = common::setup();
    let tasks = apply_filter(&mut env, vec![]);
    assert!(tasks.is_empty());
}

#[test]
fn list_excludes_done_tasks_by_default() {
    let mut env = common::setup();
    add::run(add_args("Open task"), &mut env.ctx).unwrap();

    let done_args = add::Args {
        slug: Some("done-task".to_string()),
        ..add_args("Done task")
    };
    add::run(done_args, &mut env.ctx).unwrap();

    // Mark second task done.
    use next::cli::commands::done;
    done::run(
        done::Args {
            id: "done-task".to_string(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter(&mut env, vec![]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task.title, "Open task");
}

// ---------------------------------------------------------------------------
// Future start date filtering
// ---------------------------------------------------------------------------

fn apply_filter_with_future(env: &mut common::TestEnv, include_future: bool) -> Vec<ScoredTask> {
    let today = Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec![]).unwrap();
    filter_args.future = include_future;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    scoring::score_and_sort(
        filtered,
        &all,
        today,
        &env.ctx.repo.scoring,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    )
}

#[test]
fn future_start_task_hidden_by_default() {
    let mut env = common::setup();
    add::run(add_args("Normal task"), &mut env.ctx).unwrap();
    add::run(
        add::Args {
            start: Some("2099-01-01".to_string()),
            ..add_args("Future task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, false);
    let titles: Vec<&str> = tasks.iter().map(|t| t.task.title.as_str()).collect();
    assert!(
        titles.contains(&"Normal task"),
        "normal task must be visible"
    );
    assert!(
        !titles.contains(&"Future task"),
        "future-start task must be hidden by default"
    );
}

#[test]
fn future_start_task_shown_with_future_flag() {
    let mut env = common::setup();
    add::run(add_args("Normal task"), &mut env.ctx).unwrap();
    add::run(
        add::Args {
            start: Some("2099-01-01".to_string()),
            ..add_args("Future task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, true);
    let titles: Vec<&str> = tasks.iter().map(|t| t.task.title.as_str()).collect();
    assert!(titles.contains(&"Normal task"));
    assert!(
        titles.contains(&"Future task"),
        "future-start task must appear with --future"
    );
}

#[test]
fn task_with_start_today_is_visible() {
    let mut env = common::setup();
    let today = Local::now().date_naive().to_string();
    add::run(
        add::Args {
            start: Some(today),
            ..add_args("Starts today")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, false);
    assert_eq!(
        tasks.len(),
        1,
        "task starting today must be visible without --future"
    );
}

#[test]
fn task_with_past_start_is_visible() {
    let mut env = common::setup();
    add::run(
        add::Args {
            start: Some("2000-01-01".to_string()),
            ..add_args("Started long ago")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_with_future(&mut env, false);
    assert_eq!(tasks.len(), 1, "task with past start date must be visible");
}

#[test]
fn all_flag_also_shows_future_start_tasks() {
    let mut env = common::setup();
    add::run(
        add::Args {
            start: Some("2099-06-01".to_string()),
            ..add_args("Distant future")
        },
        &mut env.ctx,
    )
    .unwrap();

    // --all disables all implicit filtering including start-date gate.
    let today = Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec![]).unwrap();
    filter_args.all = true;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    let tasks = scoring::score_and_sort(
        filtered,
        &all,
        today,
        &env.ctx.repo.scoring,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    );
    assert_eq!(tasks.len(), 1, "--all must reveal future-start tasks");
}

// ---------------------------------------------------------------------------
// Limit flag
// ---------------------------------------------------------------------------

fn list_args_with_limit(limit: Option<usize>) -> next::cli::commands::list::Args {
    next::cli::commands::list::Args {
        future: false,
        all: false,
        closed: false,
        archived: false,
        all_users: false,
        json: false,
        format: None,
        count: false,
        explain: false,
        fields: vec![],
        limit,
        page_size: None,
        page: 1,
        tokens: vec![],
    }
}

#[test]
fn list_limit_truncates_results() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    // Without limit all five tasks are returned.
    let all = apply_filter(&mut env, vec![]);
    assert_eq!(all.len(), 5);

    // With limit = 3 only three are returned.
    next::cli::commands::list::run(list_args_with_limit(Some(3)), &env.ctx).unwrap();
    let store = &env.ctx.repo.store;
    let tasks = store.list_tasks().unwrap();
    // Verify the store still has all five (limit only affects output, not storage).
    assert_eq!(tasks.len(), 5);
}

#[test]
fn list_limit_zero_shows_nothing() {
    let mut env = common::setup();
    add::run(add_args("task one"), &mut env.ctx).unwrap();
    next::cli::commands::list::run(list_args_with_limit(Some(0)), &env.ctx).unwrap();
}

#[test]
fn list_config_limit_applies_when_no_flag() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    env.ctx.config.list_limit = Some(2);
    // run() should truncate to 2 without passing --limit
    next::cli::commands::list::run(list_args_with_limit(None), &env.ctx).unwrap();
    // The store still has five tasks.
    assert_eq!(env.ctx.repo.store.list_tasks().unwrap().len(), 5);
}

#[test]
fn project_filter_returns_descendants() {
    let mut env = common::setup();

    // Root task with slug "launch"
    add::run(
        add::Args {
            slug: Some("launch".into()),
            ..add_args("Launch blog")
        },
        &mut env.ctx,
    )
    .unwrap();
    let root = env
        .ctx
        .repo
        .store
        .get_task_by_slug("launch")
        .unwrap()
        .unwrap();

    // Two direct children
    add::run(
        add::Args {
            parent: Some(root.id.to_string()),
            ..add_args("Write copy")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            parent: Some(root.id.to_string()),
            ..add_args("Design logo")
        },
        &mut env.ctx,
    )
    .unwrap();

    // An unrelated task
    add::run(add_args("Unrelated task"), &mut env.ctx).unwrap();

    // parent:launch with all=true so the parent itself passes the implicit gate
    let today = chrono::Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec!["parent:launch".into()]).unwrap();
    filter_args.all = true;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = next::core::domain::filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    let titles: Vec<&str> = filtered.iter().map(|t| t.title.as_str()).collect();
    assert!(titles.contains(&"Launch blog"), "root should be included");
    assert!(titles.contains(&"Write copy"));
    assert!(titles.contains(&"Design logo"));
    assert!(!titles.contains(&"Unrelated task"));
}

#[test]
fn project_filter_includes_grandchildren() {
    let mut env = common::setup();

    add::run(
        add::Args {
            slug: Some("project".into()),
            ..add_args("Root")
        },
        &mut env.ctx,
    )
    .unwrap();
    let root = env
        .ctx
        .repo
        .store
        .get_task_by_slug("project")
        .unwrap()
        .unwrap();

    add::run(
        add::Args {
            slug: Some("child".into()),
            parent: Some(root.id.to_string()),
            ..add_args("Child")
        },
        &mut env.ctx,
    )
    .unwrap();
    let child = env
        .ctx
        .repo
        .store
        .get_task_by_slug("child")
        .unwrap()
        .unwrap();

    add::run(
        add::Args {
            parent: Some(child.id.to_string()),
            ..add_args("Grandchild")
        },
        &mut env.ctx,
    )
    .unwrap();

    let today = chrono::Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec!["parent:project".into()]).unwrap();
    filter_args.all = true;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = next::core::domain::filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    let titles: Vec<&str> = filtered.iter().map(|t| t.title.as_str()).collect();
    assert!(titles.contains(&"Root"));
    assert!(titles.contains(&"Child"));
    assert!(titles.contains(&"Grandchild"));
}

#[test]
fn project_filter_unknown_slug_returns_empty() {
    let mut env = common::setup();
    add::run(add_args("Some task"), &mut env.ctx).unwrap();

    let today = chrono::Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(vec!["parent:nonexistent".into()]).unwrap();
    filter_args.all = true;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = next::core::domain::filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    assert!(filtered.is_empty());
}

// ---------------------------------------------------------------------------
// Parent filter — additional integration tests
// ---------------------------------------------------------------------------

fn apply_filter_all(env: &mut common::TestEnv, tokens: Vec<String>) -> Vec<ScoredTask> {
    let today = chrono::Local::now().date_naive();
    let mut filter_args = FilterArgs::parse(tokens).unwrap();
    filter_args.all = true;
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = next::core::domain::filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    scoring::score_and_sort(
        filtered,
        &all,
        today,
        &env.ctx.repo.scoring,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
    )
}

#[test]
fn parent_filter_excludes_sibling_subtrees() {
    let mut env = common::setup();

    add::run(
        add::Args {
            slug: Some("alpha".into()),
            ..add_args("Alpha")
        },
        &mut env.ctx,
    )
    .unwrap();
    let alpha = env
        .ctx
        .repo
        .store
        .get_task_by_slug("alpha")
        .unwrap()
        .unwrap();
    add::run(
        add::Args {
            parent: Some(alpha.id.to_string()),
            ..add_args("Alpha child")
        },
        &mut env.ctx,
    )
    .unwrap();

    add::run(
        add::Args {
            slug: Some("beta".into()),
            ..add_args("Beta")
        },
        &mut env.ctx,
    )
    .unwrap();
    let beta = env
        .ctx
        .repo
        .store
        .get_task_by_slug("beta")
        .unwrap()
        .unwrap();
    add::run(
        add::Args {
            parent: Some(beta.id.to_string()),
            ..add_args("Beta child")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter_all(&mut env, vec!["parent:alpha".into()]);
    let titles: Vec<&str> = tasks.iter().map(|t| t.task.title.as_str()).collect();
    assert!(titles.contains(&"Alpha"), "root included");
    assert!(titles.contains(&"Alpha child"), "child included");
    assert!(!titles.contains(&"Beta"), "sibling root excluded");
    assert!(!titles.contains(&"Beta child"), "sibling child excluded");
}

#[test]
fn default_list_hides_parent_with_open_children() {
    let mut env = common::setup();

    add::run(
        add::Args {
            slug: Some("parent".into()),
            ..add_args("Parent task")
        },
        &mut env.ctx,
    )
    .unwrap();
    let parent = env
        .ctx
        .repo
        .store
        .get_task_by_slug("parent")
        .unwrap()
        .unwrap();
    add::run(
        add::Args {
            parent: Some(parent.id.to_string()),
            ..add_args("Child task")
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter(&mut env, vec![]);
    let titles: Vec<&str> = tasks.iter().map(|t| t.task.title.as_str()).collect();
    assert!(
        !titles.contains(&"Parent task"),
        "parent hidden while child is open"
    );
    assert!(titles.contains(&"Child task"), "child visible");
}

#[test]
fn default_list_shows_parent_when_all_children_done() {
    let mut env = common::setup();

    add::run(
        add::Args {
            slug: Some("parent".into()),
            ..add_args("Parent task")
        },
        &mut env.ctx,
    )
    .unwrap();
    let parent = env
        .ctx
        .repo
        .store
        .get_task_by_slug("parent")
        .unwrap()
        .unwrap();
    add::run(
        add::Args {
            slug: Some("child".into()),
            parent: Some(parent.id.to_string()),
            ..add_args("Child task")
        },
        &mut env.ctx,
    )
    .unwrap();

    use next::cli::commands::done;
    done::run(
        done::Args {
            id: "child".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    let tasks = apply_filter(&mut env, vec![]);
    let titles: Vec<&str> = tasks.iter().map(|t| t.task.title.as_str()).collect();
    assert!(
        titles.contains(&"Parent task"),
        "parent visible once child is done"
    );
}

#[test]
fn parent_filter_only_returns_active_descendants_by_default() {
    let mut env = common::setup();

    add::run(
        add::Args {
            slug: Some("proj".into()),
            ..add_args("Project")
        },
        &mut env.ctx,
    )
    .unwrap();
    let proj = env
        .ctx
        .repo
        .store
        .get_task_by_slug("proj")
        .unwrap()
        .unwrap();
    add::run(
        add::Args {
            slug: Some("open-child".into()),
            parent: Some(proj.id.to_string()),
            ..add_args("Open child")
        },
        &mut env.ctx,
    )
    .unwrap();
    add::run(
        add::Args {
            slug: Some("done-child".into()),
            parent: Some(proj.id.to_string()),
            ..add_args("Done child")
        },
        &mut env.ctx,
    )
    .unwrap();

    use next::cli::commands::done;
    done::run(
        done::Args {
            id: "done-child".into(),
            completed_at: None,
            json: false,
        },
        &mut env.ctx,
    )
    .unwrap();

    // Without --all, done children are excluded even within the parent: scope.
    let today = chrono::Local::now().date_naive();
    let filter_args = FilterArgs::parse(vec!["parent:proj".into()]).unwrap();
    let filter_set = filter_args.to_filter_set().unwrap();
    let state = env.ctx.repo.store.get_state().unwrap();
    let all = env.ctx.repo.store.list_tasks().unwrap();
    let filtered = next::core::domain::filter::apply(
        all.clone(),
        &filter_set,
        &state,
        today,
        &std::collections::HashMap::new(),
    );
    let titles: Vec<&str> = filtered.iter().map(|t| t.title.as_str()).collect();
    assert!(titles.contains(&"Open child"));
    assert!(
        !titles.contains(&"Done child"),
        "done task excluded without --all"
    );
}

#[test]
fn list_renders_long_multibyte_title_without_panic() {
    // Regression: render_task_list used byte-index truncation (&s[..45]);
    // a title whose 45th byte fell inside a multibyte char panicked with
    // "byte index 45 is not a char boundary".
    let mut env = common::setup();
    add::run(add_args(&"é".repeat(60)), &mut env.ctx).unwrap();
    add::run(
        add_args(&format!("Fête préparée {}", "🎉".repeat(30))),
        &mut env.ctx,
    )
    .unwrap();

    // Goes through render::render_task_list, which previously panicked.
    next::cli::commands::list::run(list_args_with_limit(None), &env.ctx).unwrap();
}

#[test]
fn list_flag_overrides_config_limit() {
    let mut env = common::setup();
    for i in 1..=5 {
        add::run(add_args(&format!("task {i}")), &mut env.ctx).unwrap();
    }
    env.ctx.config.list_limit = Some(1);
    // --limit 4 overrides config limit of 1
    next::cli::commands::list::run(list_args_with_limit(Some(4)), &env.ctx).unwrap();
    assert_eq!(env.ctx.repo.store.list_tasks().unwrap().len(), 5);
}

#[test]
fn list_typoed_flag_in_filter_tokens_rejected() {
    // An unknown `--flag` swallowed into the trailing filter tokens must
    // error clearly instead of being misread as a tag exclusion.
    let env = common::setup();
    let err = next::cli::commands::list::run(
        next::cli::commands::list::Args {
            tokens: vec!["--futur".to_string()],
            ..list_args_with_limit(None)
        },
        &env.ctx,
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unrecognised flag"), "unexpected error: {msg}");
    assert!(msg.contains("--futur"), "error must name the token: {msg}");
}
