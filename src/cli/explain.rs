//! `next list --explain`: what a filter actually became.
//!
//! Two jobs. The first is answering "why did that return nothing?" without
//! reading Rust — overwhelmingly because a bare word was read as a search
//! rather than as a tag, which is the change most likely to surprise someone
//! who used the old token syntax. The second is giving a bug report something
//! concrete to paste.
//!
//! It reports what is *decided* rather than re-deriving anything: the parsed
//! expression, the terms lifted out of it, which atoms SQL can answer, and the
//! candidate count at each step. If this and the real pipeline could disagree,
//! the output would be worse than useless, so both read the same values.

use std::fmt::Write as _;

use crate::core::domain::filter::FilterSet;
use crate::core::domain::filter_expr::{Atom, Expr, TextField};
use crate::core::domain::tag::validate_tag;

/// Where a query's work actually happened.
///
/// The two tiers run differently and saying so is the point: the active list
/// pushes only the status gate and evaluates the expression in memory, while
/// the archived tier compiles as much of the expression as SQL can answer.
/// Reporting "nothing was pushed" for both would be true of neither.
pub enum Execution {
    /// The active list: candidates loaded by status, expression evaluated in
    /// memory over them.
    InMemory,
    /// The archived tier: `sql` is the compiled `WHERE`, and `exact` says
    /// whether it is the answer or merely narrows the rows to re-check.
    Sql { sql: String, exact: bool },
}

/// Renders the explanation for a parsed query.
///
/// `candidates` and `matched` are the real counts from the run that produced
/// them, and `execution` describes how that run was carried out. `candidates`
/// is `None` when the caller genuinely cannot know it — the archived tier
/// re-checks an inexact pushdown inside the store and reports only the
/// survivors — and saying so beats printing `matched` twice under two labels.
pub fn render(
    raw_query: &str,
    filter: &FilterSet,
    candidates: Option<usize>,
    matched: usize,
    execution: &Execution,
) -> String {
    let mut out = String::new();

    // The shell-ate-my-query case. An empty expression matches everything, so
    // without saying so the command looks like it ignored the filter — and the
    // usual cause is quoting the whole thing away or a stray shell glob.
    if raw_query.trim().is_empty() {
        out.push_str("No filter was given, so every task is a match.\n");
        out.push_str(
            "  If you did pass one, the shell may have consumed it — quote it:\n\
             \x20   next list '+@work and due<+7d'\n\n",
        );
    } else {
        let _ = writeln!(out, "Query:    {raw_query}");
        let _ = writeln!(out, "Parsed:   {}", filter.expr);
        out.push('\n');
    }

    // The bare-token break, made visible. This is the single most common
    // reason a query surprises someone, so it is called out by name rather
    // than left to be inferred from the AST above.
    //
    // Only bare words qualify: someone who wrote `title:foo` named the field on
    // purpose and knows it searched, so warning them would be noise.
    let bare: Vec<SearchTerm> = search_terms(&filter.expr)
        .into_iter()
        .filter(|s| s.field.is_none())
        .collect();
    if !bare.is_empty() {
        out.push_str("Read as SEARCHES (text, not tags):\n");
        for term in &bare {
            let _ = writeln!(out, "  {}", term.rendered());
        }
        out.push_str("  A bare word searches the title, description, notes and url.\n");
        // Only offered when it is a query the user can actually run: a tag
        // named `cold tier` or `arch*` cannot exist, and suggesting one is
        // worse than saying nothing.
        if let Some(tag) = bare.iter().find_map(SearchTerm::as_tag) {
            let _ = writeln!(out, "  For the TAG of that name, write +{tag}.");
        }
        out.push('\n');
    }

    // The view terms, which are not per-task predicates and are easy to forget
    // are in force at all.
    let mut scope: Vec<String> = Vec::new();
    if let Some(slug) = &filter.parent_slug {
        scope.push(format!("parent:{slug} (this project's subtree)"));
    }
    if let Some(tags) = &filter.required_override {
        scope.push(format!(
            "context: {} (for this query only)",
            tags.join(", ")
        ));
    }
    if let Some(users) = &filter.user_override {
        scope.push(match users.is_empty() {
            true => "all users (--all-users)".to_owned(),
            false => format!("user: {}", users.join(", ")),
        });
    }
    if !scope.is_empty() {
        out.push_str("Scoped to:\n");
        for s in &scope {
            let _ = writeln!(out, "  {s}");
        }
        out.push('\n');
    }

    // Which half ran where. `exact` decides whether SQL owned the answer or
    // merely narrowed it, which is exactly what someone debugging a slow or
    // surprising query wants to know.
    out.push_str("Execution:\n");
    match execution {
        Execution::InMemory => {
            out.push_str("  pushed into SQL:   the status gate only\n");
            out.push_str("  the filter itself: evaluated in memory\n");
        }
        Execution::Sql { sql, exact } => {
            let _ = writeln!(out, "  pushed into SQL:   {sql}");
            if *exact {
                out.push_str(
                    "  the filter itself: fully answered by SQL, which also \
                     paginates\n",
                );
            } else {
                out.push_str(
                    "  the filter itself: SQL narrows the rows, then every one \
                     is re-checked\n",
                );
            }
        }
    }
    match candidates {
        Some(n) => {
            let _ = writeln!(out, "  candidates loaded: {n}");
        }
        None => out.push_str(
            "  candidates loaded: not counted (the store re-checks the rows \
             and reports only the matches)\n",
        ),
    }
    let _ = writeln!(out, "  matched:           {matched}");

    // Only the active list applies the implicit gate, and only there is `--all`
    // accepted: `--archived` declares it a conflict, so offering it on the
    // archived tier is advice that cannot be taken.
    if matches!(execution, Execution::InMemory) && matched == 0 && candidates.is_some_and(|c| c > 0)
    {
        out.push_str(
            "\nNothing matched. The implicit gate hides closed tasks, future\n\
             start dates, blocked tasks and parents with open subtasks —\n\
             try --all to see past it.\n",
        );
    }

    out
}

/// One search atom, kept whole rather than pre-rendered because two different
/// questions are asked of it: what to print, and whether the user could have
/// meant a tag by it.
struct SearchTerm {
    /// `None` for a bare word — the only shape the "you meant a tag" block is
    /// about, since naming a field is never the bare-token mistake.
    field: Option<TextField>,
    term: String,
    prefix: bool,
}

impl SearchTerm {
    /// The term the way the user would have to type it.
    fn rendered(&self) -> String {
        let mut out = String::new();
        if let Some(f) = &self.field {
            out.push_str(f.name());
            out.push(':');
        }
        if self.term.contains(' ') {
            let _ = write!(out, "\"{}\"", self.term);
        } else {
            out.push_str(&self.term);
        }
        if self.prefix {
            out.push('*');
        }
        out
    }

    /// The tag this term could have been, or `None` when no tag may carry that
    /// name — so that `+X` is never suggested unless `+X` parses.
    ///
    /// A prefix search is excluded outright: `arch*` asked to match by prefix
    /// and `+arch` is an exact tag lookup, so offering it would answer a
    /// question that was not asked.
    fn as_tag(&self) -> Option<&str> {
        if self.prefix || self.field.is_some() {
            return None;
        }
        validate_tag(&self.term).ok()?;
        Some(&self.term)
    }
}

/// Every search term in the expression, in source order.
fn search_terms(expr: &Expr) -> Vec<SearchTerm> {
    let mut found = Vec::new();
    collect_searches(expr, &mut found);
    found
}

fn collect_searches(expr: &Expr, out: &mut Vec<SearchTerm>) {
    match expr {
        Expr::And(parts) | Expr::Or(parts) => {
            for p in parts {
                collect_searches(p, out);
            }
        }
        Expr::Not(inner) => collect_searches(inner, out),
        Expr::Atom(Atom::Search {
            field,
            term,
            prefix,
        }) => out.push(SearchTerm {
            field: *field,
            term: term.clone(),
            prefix: *prefix,
        }),
        Expr::Atom(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::filter_expr::parse;

    fn set(query: &str) -> FilterSet {
        let args = crate::core::FilterArgs::parse_query(query).unwrap();
        args.to_filter_set().unwrap()
    }

    #[test]
    fn an_empty_query_says_the_shell_may_have_eaten_it() {
        let out = render("", &set(""), Some(10), 10, &Execution::InMemory);
        assert!(out.contains("No filter was given"), "{out}");
        assert!(out.contains("shell may have consumed it"), "{out}");
    }

    #[test]
    fn a_bare_word_is_reported_as_a_search_with_the_tag_spelling() {
        // The whole reason --explain exists: someone typing the old token
        // syntax needs to see that `bug` searched instead of selecting a tag.
        let out = render("bug", &set("bug"), Some(10), 0, &Execution::InMemory);
        assert!(out.contains("Read as SEARCHES"), "{out}");
        assert!(out.contains("write +bug"), "{out}");
    }

    #[test]
    fn a_tag_query_is_not_reported_as_a_search() {
        let out = render("+bug", &set("+bug"), Some(10), 3, &Execution::InMemory);
        assert!(!out.contains("Read as SEARCHES"), "{out}");
    }

    #[test]
    fn the_parsed_form_round_trips_the_expression() {
        let out = render(
            "+@work and due<+7d",
            &set("+@work and due<+7d"),
            Some(5),
            2,
            &Execution::Sql {
                sql: "(tag AND due)".to_owned(),
                exact: true,
            },
        );
        assert!(out.contains("Parsed:"), "{out}");
        assert!(out.contains("+@work"), "{out}");
        assert!(out.contains("due<+7d"), "{out}");
        assert!(out.contains("pushed into SQL:   (tag AND due)"), "{out}");
        assert!(out.contains("fully answered by SQL"), "{out}");
        assert!(out.contains("candidates loaded: 5"), "{out}");
        assert!(out.contains("matched:           2"), "{out}");
    }

    /// The distinction that matters for a slow query: an inexact fragment only
    /// narrows the rows, and every one of them is re-checked afterwards.
    #[test]
    fn an_inexact_pushdown_says_the_rows_are_re_checked() {
        let out = render(
            "data.k:v",
            &set("data.k:v"),
            Some(9),
            1,
            &Execution::Sql {
                sql: "1".to_owned(),
                exact: false,
            },
        );
        assert!(out.contains("SQL narrows the rows"), "{out}");
        assert!(!out.contains("fully answered"), "{out}");
    }

    /// The active list never pushes the expression, only the status gate —
    /// saying "nothing was pushed" would be wrong in the other direction.
    #[test]
    fn the_active_list_says_where_the_filter_actually_ran() {
        let out = render("+bug", &set("+bug"), Some(3), 1, &Execution::InMemory);
        assert!(out.contains("the status gate only"), "{out}");
        assert!(out.contains("evaluated in memory"), "{out}");
        assert!(
            !out.contains("no cache"),
            "the old misleading wording: {out}"
        );
    }

    #[test]
    fn view_terms_are_reported_because_they_are_easy_to_forget() {
        let out = render(
            "parent:infra +bug",
            &set("parent:infra +bug"),
            Some(4),
            1,
            &Execution::InMemory,
        );
        assert!(out.contains("Scoped to:"), "{out}");
        assert!(out.contains("parent:infra"), "{out}");
    }

    #[test]
    fn an_empty_result_points_at_the_implicit_gate() {
        let out = render("+bug", &set("+bug"), Some(7), 0, &Execution::InMemory);
        assert!(out.contains("implicit gate"), "{out}");
        assert!(out.contains("--all"), "{out}");
    }

    /// The gate belongs to the active list alone. On the archived tier there is
    /// no gate to blame, and `--all` is a clap conflict with `--archived`, so
    /// the hint would be an instruction to run a command that errors out.
    #[test]
    fn an_empty_sql_result_does_not_blame_a_gate_it_never_applied() {
        for candidates in [Some(7), Some(0), None] {
            let out = render(
                "+bug",
                &set("+bug"),
                candidates,
                0,
                &Execution::Sql {
                    sql: "1".to_owned(),
                    exact: false,
                },
            );
            assert!(!out.contains("implicit gate"), "{candidates:?}: {out}");
            assert!(!out.contains("try --all"), "{candidates:?}: {out}");
        }
    }

    /// An inexact pushdown reads a superset it never counts back out. Printing
    /// the match count under "candidates loaded" would fake the one figure that
    /// shows how far the pushdown widened.
    #[test]
    fn an_uncounted_candidate_set_says_so_instead_of_repeating_the_match_count() {
        let out = render(
            "data.k:v",
            &set("data.k:v"),
            None,
            3,
            &Execution::Sql {
                sql: "1".to_owned(),
                exact: false,
            },
        );
        assert!(out.contains("candidates loaded: not counted"), "{out}");
        assert!(out.contains("matched:           3"), "{out}");
        assert!(!out.contains("candidates loaded: 3"), "{out}");
    }

    #[test]
    fn a_scoped_search_and_a_prefix_render_the_way_they_were_typed() {
        let expr = parse("title:arch* notes:\"cold tier\"").unwrap();
        let rendered: Vec<String> = search_terms(&expr)
            .iter()
            .map(SearchTerm::rendered)
            .collect();
        assert_eq!(rendered, vec!["title:arch*", "notes:\"cold tier\""]);
    }

    /// Naming the field is not the bare-token mistake, so the block that warns
    /// about it has nothing to say — and `+title:foo` is not even a query.
    #[test]
    fn a_scoped_search_is_not_reported_as_the_bare_token_mistake() {
        let out = render(
            "title:foo",
            &set("title:foo"),
            Some(10),
            0,
            &Execution::InMemory,
        );
        assert!(!out.contains("Read as SEARCHES"), "{out}");
        assert!(!out.contains("write +"), "{out}");
    }

    /// The block still names what searched — that part is useful — but a term
    /// no tag could be named after gets no suggestion.
    #[test]
    fn a_term_that_cannot_be_a_tag_gets_the_block_without_a_suggestion() {
        for query in ["\"cold tier\"", "arch*", "café"] {
            let out = render(query, &set(query), Some(10), 0, &Execution::InMemory);
            assert!(out.contains("Read as SEARCHES"), "{query}: {out}");
            assert!(!out.contains("write +"), "{query}: {out}");
        }
    }

    /// Everything this module suggests has to parse as the tag it claims to be.
    #[test]
    fn every_suggested_tag_parses_as_a_tag() {
        for query in ["bug", "wifi-router", "arch*", "\"cold tier\"", "café"] {
            let out = render(query, &set(query), Some(10), 0, &Execution::InMemory);
            let Some(rest) = out.split("write +").nth(1) else {
                continue;
            };
            let suggested: String = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '.')
                .collect();
            parse(&format!("+{suggested}"))
                .unwrap_or_else(|e| panic!("{query}: `+{suggested}` does not parse: {e}"));
            validate_tag(&suggested)
                .unwrap_or_else(|e| panic!("{query}: `+{suggested}` is not a tag: {e}"));
        }
    }

    /// A bare word beside a scoped one still earns the suggestion: the bare
    /// one is the mistake, and it is the one that gets named.
    #[test]
    fn the_suggestion_names_the_bare_word_not_the_scoped_one() {
        let query = "title:arch bug";
        let out = render(query, &set(query), Some(10), 0, &Execution::InMemory);
        assert!(out.contains("Read as SEARCHES"), "{out}");
        assert!(out.contains("write +bug"), "{out}");
        assert!(!out.contains("  title:arch\n"), "{out}");
    }
}
