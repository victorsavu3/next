//! The filter examples in the documentation must actually parse.
//!
//! Docs written from a design rather than from the built behaviour are the
//! usual source of drift, and filter syntax is especially prone to it: an
//! example that was valid three stages ago still *reads* fine. This walks the
//! Markdown, pulls out every filter expression it can identify, and puts it
//! through the real parser.
//!
//! It is deliberately conservative about what it treats as an example — a
//! false positive here would be a test failing over prose — so it only takes
//! command lines it can recognise unambiguously. Better to check fewer
//! examples reliably than to guess.

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
