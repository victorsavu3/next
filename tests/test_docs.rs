//! The examples in the documentation must actually parse.
//!
//! Docs written from a design rather than from the built behaviour are the
//! usual source of drift, and syntax is especially prone to it: an example
//! that was valid three stages ago still *reads* fine. This walks the
//! Markdown, pulls out every expression it can identify, and puts it through
//! the real parser.
//!
//! It covers two grammars. **Filters** — the query language every listing
//! command takes. **Recurrence** — the RRULE strings, snap values, completion
//! intervals and snap leeway specs, which drift the same way and were found to
//! have drifted badly: a review of the recurrence docs turned up ten claims
//! that the code contradicted, including RRULE syntax documented as rejected
//! that has always worked. Checking the examples is what stops an eleventh.
//! The §Problem dates in CLI.md and TUTORIAL.md are asserted too, through
//! `spawn_next`, because a table of dates is a claim like any other.
//!
//! It is deliberately conservative about what it treats as an example — a
//! false positive here would be a test failing over prose — so it only takes
//! command lines it can recognise unambiguously. Better to check fewer
//! examples reliably than to guess. Both extractors have their own tests
//! below, since one that silently stopped recognising anything would leave
//! this file passing while checking nothing.

use next::core::FilterArgs;

/// Every doc that documents filter syntax.
const DOCS: &[&str] = &[
    "CLI.md",
    "README.md",
    "TUI.md",
    "REQUIREMENTS.md",
    "CHANGELOG.md",
    "TUTORIAL.md",
    // ARCHITECTURE.md describes the filter pipeline, so its examples are as
    // able to go stale as any other doc's — and nothing else checks that file.
    "ARCHITECTURE.md",
];

/// The commands whose trailing arguments are a filter expression.
const FILTER_COMMANDS: &[&str] = &["next list", "next next", "next tree", "next forecast"];

/// Flags that take a value, so the value is not part of the filter.
const VALUE_FLAGS: &[&str] = &[
    "--page",
    "--page-size",
    "--days",
    "--format",
    "-n",
    "--limit",
    // Without this the field list of `--fields id,title,due` fell through into
    // the filter and parsed as a bare-word search, so the example "passed" as
    // a query nobody wrote.
    "--fields",
];

/// Pulls the filter expression out of one `next …` command line.
///
/// Returns `None` when the line has no filter part, which is the common case —
/// `next list --all` is a command with no query.
fn filter_from_command(line: &str) -> Option<String> {
    let line = line.trim();
    // Strip a shell comment, which several examples carry.
    let line = line.split(" #").next().unwrap_or(line).trim();
    let rest = FILTER_COMMANDS
        .iter()
        .find_map(|cmd| line.strip_prefix(cmd))?
        .trim();

    // Walk the arguments, dropping flags (and the values they consume).
    let mut parts: Vec<String> = Vec::new();
    let mut skip_next = false;
    for token in shell_split(rest) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if token.starts_with("--") || token == "-n" {
            if VALUE_FLAGS.contains(&token.as_str()) {
                skip_next = true;
            }
            continue;
        }
        parts.push(token);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" "))
}

/// Splits a command line the way a shell would, honouring single and double
/// quotes and keeping the double quotes — they are part of the filter grammar
/// (a phrase), where single quotes are only shell armour.
fn shell_split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for c in line.chars() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => {
                in_double = !in_double;
                current.push(c);
            }
            ' ' if !in_single && !in_double => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Every filter expression appearing in a command example across the docs.
fn documented_filters() -> Vec<(String, String, usize)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    for doc in DOCS {
        let path = root.join(doc);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // A doc that does not exist is not this test's problem.
        };
        for (i, line) in text.lines().enumerate() {
            if let Some(filter) = filter_from_command(line) {
                found.push(((*doc).to_owned(), filter, i + 1));
            }
        }
    }
    found
}

#[test]
fn every_documented_filter_example_parses() {
    let examples = documented_filters();
    // 26 at the time of writing. The floor guards against the extractor
    // silently ceasing to recognise command lines, which would leave this test
    // passing while checking nothing.
    assert!(
        examples.len() >= 20,
        "the extractor found only {} examples — it has probably stopped \
         recognising the command lines, which would make this test vacuous",
        examples.len()
    );

    let mut broken = Vec::new();
    for (doc, filter, line) in &examples {
        if let Err(e) = FilterArgs::parse_query(filter) {
            broken.push(format!("{doc}:{line}  {filter:?}\n    {e}"));
        }
    }
    assert!(
        broken.is_empty(),
        "documented filter examples that do not parse:\n{}",
        broken.join("\n")
    );
}

/// The extractor itself, since a silently-broken extractor would make the test
/// above pass while checking nothing.
#[test]
fn the_extractor_picks_out_the_filter_and_nothing_else() {
    assert_eq!(
        filter_from_command("next list +@work -bug"),
        Some("+@work -bug".to_owned())
    );
    assert_eq!(
        filter_from_command("next list '+@work and due<+7d'"),
        Some("+@work and due<+7d".to_owned()),
        "single quotes are shell armour and come off"
    );
    assert_eq!(
        filter_from_command("next list '\"cold tier\"'"),
        Some("\"cold tier\"".to_owned()),
        "double quotes are grammar and stay on"
    );
    assert_eq!(
        filter_from_command("next list --archived +@work --page 2"),
        Some("+@work".to_owned()),
        "flags and their values are not part of the filter"
    );
    assert_eq!(
        filter_from_command("next list -n 10 +@work"),
        Some("+@work".to_owned())
    );
    assert_eq!(
        filter_from_command("next list bug   # a trailing comment"),
        Some("bug".to_owned())
    );
    assert_eq!(
        filter_from_command("next list --json --fields id,title,due +@work"),
        Some("+@work".to_owned()),
        "a field list is a flag value, not a bare-word search"
    );
    assert_eq!(
        filter_from_command("next list --json --fields id,title,due"),
        None,
        "the field list alone leaves no filter behind"
    );
    assert_eq!(filter_from_command("next list --all"), None, "no filter");
    assert_eq!(
        filter_from_command("next add \"A task\""),
        None,
        "not a list"
    );
    assert_eq!(filter_from_command("some prose about next list"), None);
}

// ─── recurrence examples ───────────────────────────────────────────────────

use chrono::NaiveDate;
use next::core::domain::task::{Recurrence, Snap, SnapLeeway, Task};
use next::core::recurrence::{
    parse_snap, parse_snap_leeway, spawn_next, validate_recurrence, validate_rrule,
};

/// The recurrence flags whose values are a grammar of their own.
///
/// `--clear-*` take no value and so are absent by construction.
const RECUR_SCHEDULE: &str = "--recur-schedule";
const RECUR_COMPLETION: &str = "--recur-completion";
const RECUR_SNAP: &str = "--recur-snap";
const RECUR_SNAP_LEEWAY: &str = "--recur-snap-leeway";

/// Every recurrence flag value found on one documented command line.
#[derive(Default, Debug, PartialEq, Eq)]
struct RecurExample {
    schedule: Option<String>,
    completion: Option<String>,
    snap: Option<String>,
    leeway: Option<String>,
}

impl RecurExample {
    fn is_empty(&self) -> bool {
        *self == RecurExample::default()
    }
}

/// Pulls the recurrence flag values out of one logical command line.
///
/// The line-shape rule is the whole of the conservatism here: a line counts
/// only if it *is* a command — `next …` — or is a bare continuation of flags,
/// which is how CLI.md's "common patterns" block writes its RRULEs. That
/// excludes table rows (they start with `|`), headings and prose outright, so
/// the many sentences that name `--recur-snap` without giving it a value are
/// never candidates and cannot fail this test over English.
fn recurrence_from_command(line: &str) -> Option<RecurExample> {
    let line = line.trim();
    let line = line.split(" #").next().unwrap_or(line).trim();
    let is_command = line.starts_with("next ");
    let is_flag_fragment = line.starts_with("--recur-");
    if !(is_command || is_flag_fragment) {
        return None;
    }

    let mut found = RecurExample::default();
    let tokens = shell_split(line);
    let mut iter = tokens.iter().peekable();
    while let Some(token) = iter.next() {
        // Compared whole, so `--recur-snap-leeway` is never read as a
        // `--recur-snap` whose value happens to start with `-leeway`.
        let slot = match token.as_str() {
            RECUR_SCHEDULE => &mut found.schedule,
            RECUR_COMPLETION => &mut found.completion,
            RECUR_SNAP => &mut found.snap,
            RECUR_SNAP_LEEWAY => &mut found.leeway,
            _ => continue,
        };
        let Some(value) = iter.peek() else { continue };
        // A placeholder is documentation of the *shape*, not an example.
        if value.starts_with("--") || value.starts_with('<') {
            continue;
        }
        let value = value.trim_matches('"').to_owned();
        iter.next();
        if !value.is_empty() {
            *slot = Some(value);
        }
    }

    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// The docs' command examples, with backslash continuations joined so that one
/// example spanning three lines is one logical line — otherwise a rule and the
/// leeway that qualifies it would be examined separately and the cross-field
/// validation below could never run.
fn documented_recurrences() -> Vec<(String, RecurExample, usize)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    for doc in DOCS {
        let path = root.join(doc);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut pending: Option<(String, usize)> = None;
        for (i, raw) in text.lines().enumerate() {
            let (joined, start) = match pending.take() {
                Some((acc, start)) => (format!("{acc} {}", raw.trim()), start),
                None => (raw.to_owned(), i + 1),
            };
            if let Some(stripped) = joined.strip_suffix('\\') {
                pending = Some((stripped.trim_end().to_owned(), start));
                continue;
            }
            if let Some(example) = recurrence_from_command(&joined) {
                found.push(((*doc).to_owned(), example, start));
            }
        }
    }
    found
}

#[test]
fn every_documented_recurrence_example_is_accepted() {
    let examples = documented_recurrences();
    // Counted at the time of writing: 42 examples, of which 12 carry a leeway.
    // The floors guard against the extractor quietly ceasing to recognise
    // command lines, which would leave this test green while checking nothing.
    assert!(
        examples.len() >= 30,
        "the extractor found only {} recurrence examples — it has probably \
         stopped recognising the command lines, which would make this test \
         vacuous",
        examples.len()
    );
    let with_leeway = examples
        .iter()
        .filter(|(_, e, _)| e.leeway.is_some())
        .count();
    assert!(
        with_leeway >= 8,
        "only {with_leeway} documented examples set --recur-snap-leeway; the \
         extractor is probably no longer picking the value out"
    );

    let mut broken = Vec::new();
    for (doc, example, line) in &examples {
        let at = format!("{doc}:{line}");

        if let Some(rule) = &example.schedule {
            if let Err(e) = validate_rrule(rule) {
                broken.push(format!("{at}  --recur-schedule {rule:?}\n    {e}"));
            }
        }

        let snap = match &example.snap {
            Some(spec) => match parse_snap(spec) {
                Ok(snap) => Some(snap),
                Err(e) => {
                    broken.push(format!("{at}  --recur-snap {spec:?}\n    {e}"));
                    None
                }
            },
            None => None,
        };

        let leeway = match &example.leeway {
            Some(spec) => match parse_snap_leeway(spec) {
                Ok(leeway) => Some(leeway),
                Err(e) => {
                    broken.push(format!("{at}  --recur-snap-leeway {spec:?}\n    {e}"));
                    None
                }
            },
            None => None,
        };

        let interval = match &example.completion {
            Some(days) => match days.parse::<u32>() {
                Ok(n) => Some(n),
                Err(_) => {
                    broken.push(format!(
                        "{at}  --recur-completion {days:?}\n    not a whole number of days"
                    ));
                    None
                }
            },
            None => None,
        };

        // Assemble what the line describes and put it through the same checks
        // the CLI, MCP and TUI share. This is what catches a documented pairing
        // no single value is wrong in — a leeway with no snap, or a backward
        // tolerance as long as the interval it is supposed to fit inside.
        let rule = match (interval, &example.schedule) {
            (Some(interval_days), _) => Some(Recurrence::Completion {
                interval_days,
                snap: snap.clone(),
                snap_leeway: leeway.clone(),
            }),
            (None, Some(rrule)) => Some(Recurrence::Schedule {
                rrule: rrule.clone(),
                anchor: d(2026, 6, 1),
                snap: snap.clone(),
                snap_leeway: leeway.clone(),
            }),
            (None, None) => None,
        };
        if let Some(rule) = rule {
            if let Err(e) = validate_recurrence(&rule) {
                broken.push(format!("{at}  {example:?}\n    {e}"));
            }
        }
    }

    assert!(
        broken.is_empty(),
        "documented recurrence examples the code would reject:\n{}",
        broken.join("\n")
    );
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// Spawns the next instance of a `dom:1` rent task completed on `completed`.
///
/// Goes through `spawn_next` rather than reimplementing the rule, so the test
/// asserts what a user completing the task would actually get.
fn rent_next_due(
    completed: NaiveDate,
    interval_days: u32,
    leeway: Option<SnapLeeway>,
) -> NaiveDate {
    let mut task = Task::new("Pay rent");
    task.due = Some(d(2026, 6, 1));
    task.recurrence = Some(Recurrence::Completion {
        interval_days,
        snap: Some(Snap::DayOfMonth { day: 1 }),
        snap_leeway: leeway,
    });
    spawn_next(&task, completed)
        .expect("spawning must not fail")
        .expect("a recurring task spawns a successor")
        .due
        .expect("the successor inherits a due date")
}

#[test]
fn the_documented_rent_table_holds() {
    // The table printed in CLI.md, TUTORIAL.md and CHANGELOG.md, row for row:
    // interval 30, snap dom:1, with and without a leeway of 3.
    let rows = [
        (d(2026, 6, 1), d(2026, 7, 1), d(2026, 7, 1)),
        (d(2026, 6, 2), d(2026, 8, 1), d(2026, 7, 1)),
        (d(2026, 6, 5), d(2026, 8, 1), d(2026, 7, 5)),
        (d(2026, 6, 15), d(2026, 8, 1), d(2026, 7, 15)),
        (d(2026, 6, 30), d(2026, 8, 1), d(2026, 8, 1)),
    ];
    for (completed, default_due, leeway_due) in rows {
        assert_eq!(
            rent_next_due(completed, 30, None),
            default_due,
            "completing on {completed} with no leeway"
        );
        assert_eq!(
            rent_next_due(completed, 30, lw("3")),
            leeway_due,
            "completing on {completed} with --recur-snap-leeway 3"
        );
    }

    // The claim the whole feature rests on, stated on its own so a failure
    // names it: one day late must not cost a whole extra cycle.
    assert_eq!(rent_next_due(d(2026, 6, 2), 30, None), d(2026, 8, 1));
    assert_eq!(rent_next_due(d(2026, 6, 2), 30, lw("3")), d(2026, 7, 1));
}

/// Spawns from a weekly `next-workday` task completed on `completed`, which is
/// chosen so the raw date lands on the weekend.
fn workday_next_due(completed: NaiveDate, leeway: Option<SnapLeeway>) -> NaiveDate {
    let mut task = Task::new("Weekend edge");
    task.due = Some(completed);
    task.recurrence = Some(Recurrence::Completion {
        interval_days: 7,
        snap: Some(Snap::NextWorkday),
        snap_leeway: leeway,
    });
    spawn_next(&task, completed).unwrap().unwrap().due.unwrap()
}

/// A leeway written the way a user types it into `--recur-snap-leeway`.
fn lw(spec: &str) -> Option<SnapLeeway> {
    Some(parse_snap_leeway(spec).expect("a spec used in a test must parse"))
}

#[test]
fn the_documented_next_workday_table_holds() {
    // 2026-05-30 is a Saturday and 2026-05-31 a Sunday, so +7 days lands the
    // raw date on 2026-06-06 (Sat) and 2026-06-07 (Sun) respectively.
    let (fri, sat, sun, mon) = (d(2026, 6, 5), d(2026, 6, 6), d(2026, 6, 7), d(2026, 6, 8));
    let raw_sat = d(2026, 5, 30);
    let raw_sun = d(2026, 5, 31);

    // A Saturday is one day back from Friday and two forward to Monday; a
    // Sunday is the mirror. Every row of the table in CLI.md follows from that.
    assert_eq!(
        workday_next_due(raw_sat, None),
        mon,
        "default: pushes later"
    );
    assert_eq!(
        workday_next_due(raw_sun, None),
        mon,
        "default: pushes later"
    );
    assert_eq!(workday_next_due(raw_sat, lw("2")), fri, "nearest wins");
    assert_eq!(workday_next_due(raw_sun, lw("2")), mon, "nearest wins");
    assert_eq!(workday_next_due(raw_sat, lw("1")), fri);
    assert_eq!(workday_next_due(raw_sun, lw("1")), mon);
    assert_eq!(
        workday_next_due(raw_sat, lw("0,1")),
        sat,
        "Monday is two days forward, so a forward tolerance of 1 cannot reach it"
    );
    assert_eq!(workday_next_due(raw_sun, lw("0,1")), mon);
    assert_eq!(workday_next_due(raw_sat, lw("1,0")), fri);
    assert_eq!(
        workday_next_due(raw_sun, lw("1,0")),
        sun,
        "Friday is two days back, so a backward tolerance of 1 cannot reach it"
    );
    assert_eq!(workday_next_due(raw_sat, lw("0,0")), sat, "0,0 never snaps");
    assert_eq!(workday_next_due(raw_sun, lw("0,0")), sun, "0,0 never snaps");
}

#[test]
fn an_absent_leeway_is_the_pre_leeway_behaviour() {
    // The claim every doc repeats, and the reason nothing migrates: an absent
    // `snap_leeway` is `back 0, forward unbounded`, not "no snapping".
    assert_eq!(SnapLeeway::DEFAULT.back, 0);
    assert_eq!(SnapLeeway::DEFAULT.forward, None);
    for day in [1u32, 2, 5, 15, 30] {
        assert_eq!(
            rent_next_due(d(2026, 6, day), 30, None),
            rent_next_due(d(2026, 6, day), 30, Some(SnapLeeway::DEFAULT)),
            "an absent leeway must compute the same date as the default it stands for"
        );
    }
    // …and `0` is *not* that default, which is exactly what the docs warn
    // about: `0` refuses to push forward, the default pushes forward for ever.
    // A spec string cannot spell the default at all — there is no syntax for
    // "unbounded" — which is why omitting the flag is the only way to get it.
    assert_ne!(
        workday_next_due(d(2026, 5, 30), None),
        workday_next_due(d(2026, 5, 30), lw("0")),
    );
}

/// The recurrence extractor, tested the way the filter one is: a false
/// positive here would be a test failing over prose, so the shapes it must
/// *not* pick up are as load-bearing as the ones it must.
#[test]
fn the_recurrence_extractor_picks_out_the_flag_values_and_nothing_else() {
    let got = recurrence_from_command(
        "next add \"Pay rent\" --recur-completion 30 --recur-snap dom:1 --recur-snap-leeway 3",
    )
    .expect("a full command line is an example");
    assert_eq!(got.completion.as_deref(), Some("30"));
    assert_eq!(got.snap.as_deref(), Some("dom:1"));
    assert_eq!(got.leeway.as_deref(), Some("3"));
    assert_eq!(got.schedule, None);

    assert_eq!(
        recurrence_from_command("--recur-schedule \"FREQ=MONTHLY;BYMONTHDAY=-1\"")
            .unwrap()
            .schedule
            .as_deref(),
        Some("FREQ=MONTHLY;BYMONTHDAY=-1"),
        "a bare flag fragment is how the RRULE patterns are written, and the \
         surrounding double quotes are shell armour"
    );

    assert_eq!(
        recurrence_from_command("next edit rent --recur-snap-leeway 5,0   # asymmetric")
            .unwrap()
            .leeway
            .as_deref(),
        Some("5,0"),
        "a trailing comment is not part of the value"
    );

    assert_eq!(
        recurrence_from_command("next edit rent --clear-recur-snap-leeway"),
        None,
        "a clear flag takes no value and is not an example of a spec"
    );

    // The shapes that must never be picked up. Every one of these appears in
    // the docs; treating any of them as an example would fail the test above
    // over a sentence or a table cell.
    for prose in [
        "`--recur-snap-leeway` bounds that movement, in days.",
        "| `--recur-snap-leeway <spec>` | `N` or `BACK,FORWARD` | none | How far … |",
        "### Snap leeway (`--recur-snap-leeway`)",
        "Use `--recur-snap` to replace it or `--clear-recur-snap` to drop both.",
        "  `--recur-snap-leeway requires a snap; set --recur-snap first (e.g. dom:1)`",
        "The next instance is created `<days>` after the completion date.",
    ] {
        assert_eq!(
            recurrence_from_command(prose),
            None,
            "prose must not be read as an example: {prose:?}"
        );
    }

    assert_eq!(
        recurrence_from_command("next add \"Water plants\" --recur-completion <days>"),
        None,
        "a placeholder documents the shape, not a value"
    );
    assert_eq!(
        recurrence_from_command("next list +@work"),
        None,
        "a command with no recurrence flag is not a recurrence example"
    );
}
