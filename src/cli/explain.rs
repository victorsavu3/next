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
use crate::core::domain::filter_expr::{Atom, Expr};

/// Renders the explanation for a parsed query.
///
/// `candidates` and `matched` are the real counts from the run that produced
/// them; `pushed` is what the storage layer was able to compile, or `None`
/// when the query never reached a cache.
pub fn render(
    raw_query: &str,
    filter: &FilterSet,
    candidates: usize,
    matched: usize,
    pushed: Option<&str>,
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
    let searches = search_terms(&filter.expr);
    if !searches.is_empty() {
        out.push_str("Read as SEARCHES (text, not tags):\n");
        for term in &searches {
            let _ = writeln!(out, "  {term}");
        }
        let _ = writeln!(
            out,
            "  A bare word searches the title, description, notes and url.\n\
             \x20 For the TAG of that name, write +{}.\n",
            searches[0]
                .trim_matches('"')
                .split(' ')
                .next()
                .unwrap_or("tag")
        );
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
    match pushed {
        Some(sql) => {
            let _ = writeln!(out, "  pushed into SQL:  {sql}");
        }
        None => out.push_str("  pushed into SQL:  nothing (no cache for this query)\n"),
    }
    let _ = writeln!(out, "  candidates loaded: {candidates}");
    let _ = writeln!(out, "  matched:           {matched}");

    if matched == 0 && candidates > 0 {
        out.push_str(
            "\nNothing matched. The implicit gate hides closed tasks, future\n\
             start dates, blocked tasks and parents with open subtasks —\n\
             try --all to see past it.\n",
        );
    }

    out
}

/// Every search term in the expression, in source order, rendered the way the
/// user would have to type it.
fn search_terms(expr: &Expr) -> Vec<String> {
    let mut found = Vec::new();
    collect_searches(expr, &mut found);
    found
}

fn collect_searches(expr: &Expr, out: &mut Vec<String>) {
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
        }) => {
            let mut rendered = String::new();
            if let Some(f) = field {
                rendered.push_str(f.name());
                rendered.push(':');
            }
            if term.contains(' ') {
                let _ = write!(rendered, "\"{term}\"");
            } else {
                rendered.push_str(term);
            }
            if *prefix {
                rendered.push('*');
            }
            out.push(rendered);
        }
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
        let out = render("", &set(""), 10, 10, None);
        assert!(out.contains("No filter was given"), "{out}");
        assert!(out.contains("shell may have consumed it"), "{out}");
    }

    #[test]
    fn a_bare_word_is_reported_as_a_search_with_the_tag_spelling() {
        // The whole reason --explain exists: someone typing the old token
        // syntax needs to see that `bug` searched instead of selecting a tag.
        let out = render("bug", &set("bug"), 10, 0, None);
        assert!(out.contains("Read as SEARCHES"), "{out}");
        assert!(out.contains("write +bug"), "{out}");
    }

    #[test]
    fn a_tag_query_is_not_reported_as_a_search() {
        let out = render("+bug", &set("+bug"), 10, 3, None);
        assert!(!out.contains("Read as SEARCHES"), "{out}");
    }

    #[test]
    fn the_parsed_form_round_trips_the_expression() {
        let out = render(
            "+@work and due<+7d",
            &set("+@work and due<+7d"),
            5,
            2,
            Some("(tag AND due)"),
        );
        assert!(out.contains("Parsed:"), "{out}");
        assert!(out.contains("+@work"), "{out}");
        assert!(out.contains("due<+7d"), "{out}");
        assert!(out.contains("pushed into SQL:  (tag AND due)"), "{out}");
        assert!(out.contains("candidates loaded: 5"), "{out}");
        assert!(out.contains("matched:           2"), "{out}");
    }

    #[test]
    fn view_terms_are_reported_because_they_are_easy_to_forget() {
        let out = render("parent:infra +bug", &set("parent:infra +bug"), 4, 1, None);
        assert!(out.contains("Scoped to:"), "{out}");
        assert!(out.contains("parent:infra"), "{out}");
    }

    #[test]
    fn an_empty_result_points_at_the_implicit_gate() {
        let out = render("+bug", &set("+bug"), 7, 0, None);
        assert!(out.contains("implicit gate"), "{out}");
        assert!(out.contains("--all"), "{out}");
    }

    #[test]
    fn a_scoped_search_and_a_prefix_render_the_way_they_were_typed() {
        let expr = parse("title:arch* notes:\"cold tier\"").unwrap();
        let terms = search_terms(&expr);
        assert_eq!(terms, vec!["title:arch*", "notes:\"cold tier\""]);
    }
}
