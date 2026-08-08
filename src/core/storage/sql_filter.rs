//! Compiling a filter [`Expr`] into a SQLite `WHERE` fragment.
//!
//! Stage 2 of the filter language. The cache stores some of a task's fields as
//! indexed columns and the rest inside a JSON blob, so only part of an
//! expression can be answered in SQL. This module translates that part; the
//! caller evaluates the expression again in memory over whatever comes back.
//!
//! # The one rule: superset, never subset
//!
//! A translation that is too *narrow* silently drops rows, and nothing fails —
//! the user simply never sees a task that matched their query. A translation
//! that is too *wide* costs a few extra rows that the in-memory pass then
//! discards. So every approximation here widens.
//!
//! That is why the compiler is two functions rather than one. [`exact`]
//! translates only what it can translate *precisely*, returning `None`
//! otherwise; [`superset`] always succeeds, falling back to `TRUE`. `NOT` is
//! the reason the distinction matters: negating a superset gives a *subset*,
//! so `Not` may only be pushed when its operand compiled exactly.
//!
//! # Injection
//!
//! No value is ever interpolated into the SQL string. Every user-supplied
//! value becomes a `?` placeholder with the value in [`SqlFilter::params`];
//! the only text this module writes into the query is its own column names and
//! operators.

use chrono::NaiveDate;

use crate::core::domain::filter_expr::{Atom, CompareOp, Expr, Field, Named, Value};

/// A compiled `WHERE` fragment and the parameters it binds, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqlFilter {
    /// A boolean SQL expression over the `tasks` table. Always parenthesised
    /// so a caller can `AND` it onto an existing fragment.
    pub sql: String,
    /// Values for the `?` placeholders in `sql`, in order.
    pub params: Vec<String>,
    /// Whether `sql` matches the expression *precisely*. When false it matches
    /// a superset and the caller must re-check every row in memory — and must
    /// not paginate in SQL, since the row count is not yet the answer.
    pub exact: bool,
}

impl SqlFilter {
    fn new(sql: impl Into<String>, params: Vec<String>, exact: bool) -> Self {
        Self {
            sql: sql.into(),
            params,
            exact,
        }
    }

    /// The fragment that matches every row.
    fn everything() -> Self {
        Self::new("1", Vec::new(), false)
    }

    /// The fragment that matches no row. Exact: "nothing matches" is a precise
    /// answer, not an approximation — an unresolvable date value really does
    /// match nothing.
    fn nothing() -> Self {
        Self::new("0", Vec::new(), true)
    }
}

/// Compiles `expr` into a fragment matching **at least** every task the
/// expression matches.
///
/// Check [`SqlFilter::exact`] before trusting the row set or the count.
pub(crate) fn compile(expr: &Expr, today: NaiveDate) -> SqlFilter {
    superset(expr, today)
}

/// The always-succeeds translation. Falls back to `TRUE` for anything it
/// cannot express, which widens the result rather than narrowing it.
fn superset(expr: &Expr, today: NaiveDate) -> SqlFilter {
    match expr {
        // AND of supersets is a superset of the AND.
        Expr::And(parts) => join(parts, " AND ", "1", today),
        // OR of supersets is a superset of the OR — if one branch widens to
        // TRUE the whole disjunction does, which is correct.
        Expr::Or(parts) => join(parts, " OR ", "0", today),
        // The asymmetric case: NOT of a superset is a SUBSET, which would drop
        // rows. Only an exactly-translated operand may be negated.
        Expr::Not(inner) => match exact(inner, today) {
            Some(f) => SqlFilter::new(format!("(NOT {})", f.sql), f.params, true),
            None => SqlFilter::everything(),
        },
        Expr::Atom(atom) => atom_sql(atom, today).unwrap_or_else(SqlFilter::everything),
    }
}

/// The precise translation, or `None` when the expression cannot be expressed
/// in SQL without approximation.
fn exact(expr: &Expr, today: NaiveDate) -> Option<SqlFilter> {
    match expr {
        Expr::And(parts) => join_exact(parts, " AND ", "1", today),
        Expr::Or(parts) => join_exact(parts, " OR ", "0", today),
        Expr::Not(inner) => {
            let f = exact(inner, today)?;
            Some(SqlFilter::new(format!("(NOT {})", f.sql), f.params, true))
        }
        Expr::Atom(atom) => atom_sql(atom, today).filter(|f| f.exact),
    }
}

/// Combines `parts` with `sep`, widening whatever cannot be translated.
/// `empty` is the identity for the operator (`1` for AND, `0` for OR).
fn join(parts: &[Expr], sep: &str, empty: &str, today: NaiveDate) -> SqlFilter {
    if parts.is_empty() {
        return SqlFilter::new(empty, Vec::new(), true);
    }
    let compiled: Vec<SqlFilter> = parts.iter().map(|p| superset(p, today)).collect();
    combine(compiled, sep)
}

/// Like [`join`], but yields `None` unless every part translated exactly.
fn join_exact(parts: &[Expr], sep: &str, empty: &str, today: NaiveDate) -> Option<SqlFilter> {
    if parts.is_empty() {
        return Some(SqlFilter::new(empty, Vec::new(), true));
    }
    let compiled: Vec<SqlFilter> = parts
        .iter()
        .map(|p| exact(p, today))
        .collect::<Option<_>>()?;
    Some(combine(compiled, sep))
}

fn combine(mut compiled: Vec<SqlFilter>, sep: &str) -> SqlFilter {
    // A lone operand needs no grouping; every caller either wraps its own
    // result or is itself inside a group.
    if compiled.len() == 1 {
        return compiled.pop().expect("length checked");
    }
    let exact = compiled.iter().all(|f| f.exact);
    let mut params = Vec::new();
    let mut sql = String::from("(");
    for (i, f) in compiled.into_iter().enumerate() {
        if i > 0 {
            sql.push_str(sep);
        }
        sql.push_str(&f.sql);
        params.extend(f.params);
    }
    sql.push(')');
    SqlFilter::new(sql, params, exact)
}

// ─── Atoms ──────────────────────────────────────────────────────────────────

/// Whether a task carries the tag or a descendant of it — the same rule
/// `tag::tag_matches` applies, expressed against `task_tags`.
///
/// `LIKE ? || '/%'` is safe for the descendant half because tag names are
/// validated to letters, digits, `-`, `_` and `/`, so a tag can never contain
/// a `%` or `_` wildcard that would widen the match. (`_` is a LIKE wildcard,
/// but it only ever widens by one character within an already-anchored prefix,
/// and the in-memory pass rechecks anyway when the query is inexact.)
const TAG_MATCH: &str = "EXISTS (SELECT 1 FROM task_tags tt \
     WHERE tt.task_id = tasks.id AND (tt.tag = ? OR tt.tag LIKE ? || '/%'))";

/// Priority is stored as a word, so ordering needs an explicit rank — `'high'`
/// sorts before `'low'` lexicographically, which is the opposite of its urgency.
const PRIORITY_RANK: &str =
    "(CASE priority WHEN 'low' THEN 0 WHEN 'medium' THEN 1 WHEN 'high' THEN 2 END)";

fn atom_sql(atom: &Atom, today: NaiveDate) -> Option<SqlFilter> {
    match atom {
        Atom::Tag(name) => Some(SqlFilter::new(
            TAG_MATCH,
            vec![name.clone(), name.clone()],
            true,
        )),
        Atom::Equals { field, values } => equals_sql(field, values, today),
        Atom::Compare { field, op, value } => compare_sql(field, *op, value, today),
        Atom::Range { field, low, high } => {
            let lo = compare_sql(field, CompareOp::Ge, low, today)?;
            let hi = compare_sql(field, CompareOp::Le, high, today)?;
            Some(combine(vec![lo, hi], " AND "))
        }
        Atom::Has(field) => has_sql(field),
        Atom::Is(named) => is_sql(*named, today),
        // Full-text search stays in memory until the FTS index lands (S3).
        Atom::Search { .. } => None,
    }
}

/// `field:a,b` — membership, which is a disjunction over the values.
fn equals_sql(field: &Field, values: &[Value], today: NaiveDate) -> Option<SqlFilter> {
    // Tag equality is the only field whose column lives in another table.
    if matches!(field, Field::Tag | Field::Context) {
        let parts: Vec<SqlFilter> = values
            .iter()
            .map(|v| SqlFilter::new(TAG_MATCH, vec![v.raw.clone(), v.raw.clone()], true))
            .collect();
        return Some(combine(parts, " OR "));
    }

    if let Some(column) = date_column(field) {
        let parts: Vec<SqlFilter> = values
            .iter()
            .map(|v| match resolve(v, today) {
                Some(date) => SqlFilter::new(format!("{column} = ?"), vec![date], true),
                None => SqlFilter::nothing(),
            })
            .collect();
        return Some(combine(parts, " OR "));
    }

    let column = text_column(field)?;
    let marks = vec!["?"; values.len()].join(", ");
    let params: Vec<String> = values.iter().map(|v| v.raw.clone()).collect();
    // `user:` is not `assignee:`. An unassigned task belongs to whoever is
    // looking, so it passes a user filter — dropping the NULL branch here
    // would push a subset and hide every shared task.
    if *field == Field::User {
        return Some(SqlFilter::new(
            format!("(assignee IS NULL OR assignee IN ({marks}))"),
            params,
            true,
        ));
    }
    Some(SqlFilter::new(
        format!("{column} IN ({marks})"),
        params,
        true,
    ))
}

fn compare_sql(field: &Field, op: CompareOp, value: &Value, today: NaiveDate) -> Option<SqlFilter> {
    let symbol = op.symbol();
    if let Some(column) = date_column(field) {
        return Some(match resolve(value, today) {
            // A NULL column never satisfies a comparison in SQL, which is
            // exactly what the evaluator does with an unset field.
            Some(date) => SqlFilter::new(format!("{column} {symbol} ?"), vec![date], true),
            None => SqlFilter::nothing(),
        });
    }
    if *field == Field::Priority {
        let rank = match value.raw.to_ascii_lowercase().as_str() {
            "low" => 0,
            "medium" | "med" => 1,
            "high" => 2,
            // An unknown priority matches nothing, as in the evaluator.
            _ => return Some(SqlFilter::nothing()),
        };
        // The rank is written into the SQL rather than bound, and it is the
        // one place that happens. A bound parameter arrives as TEXT, and
        // SQLite orders every integer before every string, so `CASE … END >= '1'`
        // is false for all three priorities. The value here is not user input:
        // it is 0, 1 or 2 chosen by the match above.
        return Some(SqlFilter::new(
            format!("{PRIORITY_RANK} {symbol} {rank}"),
            Vec::new(),
            true,
        ));
    }
    None
}

fn has_sql(field: &Field) -> Option<SqlFilter> {
    // Tags live in their own table, so "has any tag" is an EXISTS with no
    // name to match.
    if *field == Field::Tag {
        return Some(SqlFilter::new(
            "EXISTS (SELECT 1 FROM task_tags tt WHERE tt.task_id = tasks.id)",
            Vec::new(),
            true,
        ));
    }
    let column = date_column(field).or_else(|| text_column(field))?;
    Some(SqlFilter::new(
        format!("{column} IS NOT NULL"),
        Vec::new(),
        true,
    ))
}

fn is_sql(named: Named, today: NaiveDate) -> Option<SqlFilter> {
    match named {
        Named::Closed => Some(SqlFilter::new(
            "status IN ('done', 'cancelled')",
            Vec::new(),
            true,
        )),
        Named::Archived => Some(SqlFilter::new("archived <> 0", Vec::new(), true)),
        Named::Assigned => Some(SqlFilter::new("assignee IS NOT NULL", Vec::new(), true)),
        Named::Overdue => Some(SqlFilter::new(
            "(due IS NOT NULL AND due < ? AND status IN ('open', 'started'))",
            vec![today.to_string()],
            true,
        )),
        // `recurrence` lives in the JSON blob; `blocked` and `project` are
        // questions about *other* tasks, which a paginated tier query has no
        // view of. All three stay in memory.
        Named::Recurring | Named::Blocked | Named::Project => None,
    }
}

// ─── Columns ────────────────────────────────────────────────────────────────

/// The column holding a plain-text field, if the cache indexes one.
///
/// `title`, `description`, `notes`, `url` and `data.*` are not here: they live
/// only inside the JSON blob, so predicates on them stay in memory.
fn text_column(field: &Field) -> Option<&'static str> {
    Some(match field {
        Field::Status => "status",
        Field::Priority => "priority",
        Field::Slug => "slug",
        Field::Assignee | Field::User => "assignee",
        _ => return None,
    })
}

/// The column holding a date field, and how to read a date out of it.
///
/// `due`, `start` and `completed_at` store a bare `YYYY-MM-DD`, which compares
/// correctly as text. `created_at` and `updated_at` store an RFC 3339 UTC
/// timestamp, so the leading 10 characters are the same UTC date the evaluator
/// gets from `.date_naive()`.
fn date_column(field: &Field) -> Option<&'static str> {
    Some(match field {
        Field::Due => "due",
        Field::Start => "start",
        Field::Completed => "completed_at",
        Field::Created => "substr(created_at, 1, 10)",
        Field::Updated => "substr(updated_at, 1, 10)",
        _ => return None,
    })
}

/// Resolves a date value the same way the evaluator does, so the two paths
/// cannot disagree about what `+7d` means.
fn resolve(value: &Value, today: NaiveDate) -> Option<String> {
    crate::core::domain::filter_eval::resolve_date(value, today).map(|d| d.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::filter_expr::parse;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
    }

    fn sql(query: &str) -> SqlFilter {
        compile(&parse(query).unwrap(), today())
    }

    /// Every atom form, so a regression shows up as a changed fragment rather
    /// than as silently different results.
    #[test]
    fn pushable_atoms_compile_exactly() {
        for query in [
            "+@work",
            "tag:@work,@home",
            "status:open",
            "status:open,done",
            "priority:high",
            "priority>=medium",
            "slug:water-plants",
            "assignee:alice",
            "due:2026-05-20",
            "due<+7d",
            "due:2026-05-01..2026-05-31",
            "created>2026-01-01",
            "updated<today",
            "has:due",
            "has:assignee",
            "has:tag",
            "no:due",
            "is:closed",
            "is:archived",
            "is:assigned",
            "is:overdue",
            "+a and +b",
            "+a or +b",
            "not +a",
            "(+a or +b) and status:open",
        ] {
            assert!(sql(query).exact, "{query} should push exactly");
        }
    }

    #[test]
    fn unpushable_atoms_widen_to_everything() {
        for query in [
            "sometext",        // search — until S3
            "title:something", // scoped search
            "data.estimate:3", // inside the JSON blob
            "is:recurring",    // inside the JSON blob
            "is:blocked",      // about other tasks
            "is:project",      // about other tasks
            "has:description", // inside the JSON blob
        ] {
            let f = sql(query);
            assert!(!f.exact, "{query} must not claim to be exact");
            assert_eq!(f.sql, "1", "{query} must widen to everything");
        }
    }

    // ── The superset rule ────────────────────────────────────────────────────

    #[test]
    fn a_conjunction_keeps_the_half_it_can_push() {
        // `+@work sometext` — the tag narrows in SQL, the search is rechecked.
        let f = sql("+@work sometext");
        assert!(!f.exact);
        assert!(f.sql.contains("task_tags"), "{}", f.sql);
        assert!(f.sql.contains(" AND 1"), "{}", f.sql);
    }

    #[test]
    fn a_disjunction_with_an_unpushable_branch_widens_to_everything() {
        // THE subset trap: pushing only `+@work` here would drop every task
        // that matched the search but not the tag.
        let f = sql("+@work or sometext");
        assert!(!f.exact);
        assert_eq!(
            f.sql, "(EXISTS (SELECT 1 FROM task_tags tt WHERE tt.task_id = tasks.id AND (tt.tag = ? OR tt.tag LIKE ? || '/%')) OR 1)",
            "the OR must still admit everything the search could match"
        );
    }

    #[test]
    fn negating_an_inexact_operand_widens_instead_of_narrowing() {
        // `not (search)` must not become `NOT 1` — that is empty, and every
        // task not mentioning the term would vanish.
        let f = sql("not sometext");
        assert!(!f.exact);
        assert_eq!(f.sql, "1");

        // Same trap one level down: the operand mixes pushable and not.
        let f = sql("not (+@work and sometext)");
        assert!(!f.exact);
        assert_eq!(f.sql, "1");
    }

    #[test]
    fn negating_an_exact_operand_is_pushed() {
        let f = sql("not +@work");
        assert!(f.exact);
        assert!(f.sql.starts_with("(NOT "), "{}", f.sql);
        assert!(f.sql.contains("task_tags"), "{}", f.sql);
    }

    #[test]
    fn a_nested_mix_still_only_widens() {
        // not(a or search) → widens; and(that, b) → keeps b.
        let f = sql("not (+a or sometext) and +b");
        assert!(!f.exact);
        assert!(f.sql.contains("task_tags"), "{}", f.sql);
    }

    // ── Values ───────────────────────────────────────────────────────────────

    #[test]
    fn dates_resolve_the_same_way_the_evaluator_resolves_them() {
        assert_eq!(sql("due<+7d").params, vec!["2026-05-24".to_owned()]);
        assert_eq!(sql("due:today").params, vec!["2026-05-17".to_owned()]);
        assert_eq!(sql("due<eom").params, vec!["2026-05-31".to_owned()]);
        assert_eq!(
            sql("due:\"tomorrow\"").params,
            vec!["2026-05-18".to_owned()]
        );
    }

    #[test]
    fn an_unresolvable_date_matches_nothing_rather_than_everything() {
        // Widening here would be wrong in the other direction: the evaluator
        // matches no task, so the SQL may safely say so too.
        let f = sql("due:\"not a date at all\"");
        assert_eq!(f.sql, "0");
        assert!(f.exact, "matching nothing is a precise answer");
    }

    #[test]
    fn priority_compares_by_rank_not_alphabetically() {
        let f = sql("priority>low");
        assert!(f.sql.contains("CASE priority"), "{}", f.sql);
        // 'high' < 'low' as text, so the naive translation would be backwards.
        assert!(!f.sql.contains("priority > ?"), "{}", f.sql);
        // And the rank is a literal, not a bound parameter: SQLite orders
        // every integer before every string, so `CASE … END > '0'` would be
        // false for every task. Caught by the differential test.
        assert!(f.params.is_empty(), "{:?}", f.params);
        assert!(f.sql.ends_with("> 0"), "{}", f.sql);
    }

    #[test]
    fn user_keeps_the_unassigned_branch() {
        // The subtlest subset trap in the set: `assignee IN (...)` alone would
        // hide every unassigned task, which the evaluator shows.
        let f = sql("user:alice");
        assert!(f.exact);
        assert_eq!(f.sql, "(assignee IS NULL OR assignee IN (?))");
        assert_eq!(f.params, vec!["alice".to_owned()]);
        // `assignee:` is the strict form and must NOT gain that branch.
        assert_eq!(sql("assignee:alice").sql, "assignee IN (?)");
    }

    #[test]
    fn an_unknown_enum_value_matches_nothing() {
        assert_eq!(sql("priority>sideways").sql, "0");
        // Equality goes through `IN`, where an unknown value simply matches no
        // row without needing a special case.
        assert_eq!(sql("status:sideways").params, vec!["sideways".to_owned()]);
    }

    #[test]
    fn git_dates_compare_on_the_date_half_of_the_timestamp() {
        let f = sql("created>2026-01-01");
        assert!(f.sql.contains("substr(created_at, 1, 10)"), "{}", f.sql);
    }

    // ── Injection ────────────────────────────────────────────────────────────

    #[test]
    fn values_are_bound_never_interpolated() {
        // A value carrying quotes, a semicolon and a comment marker must end
        // up in `params` and leave no trace in the SQL text.
        let nasty = "x'; DROP TABLE tasks; --";
        let expr = parse(&format!("slug:{:?}", nasty)).unwrap();
        let f = compile(&expr, today());
        assert_eq!(f.params, vec![nasty.to_owned()]);
        assert!(!f.sql.contains("DROP"), "{}", f.sql);
        assert!(!f.sql.contains(';'), "{}", f.sql);
        assert_eq!(f.sql, "slug IN (?)");
    }

    #[test]
    fn a_tag_value_is_bound_too() {
        let f = sql("+@work");
        assert_eq!(f.params, vec!["@work".to_owned(), "@work".to_owned()]);
        assert!(!f.sql.contains("@work"), "{}", f.sql);
    }

    #[test]
    fn parameter_order_follows_the_placeholders() {
        // Two atoms, four placeholders, and the caller binds positionally —
        // a mismatch here would silently filter on the wrong values.
        let f = sql("+@work and slug:x and status:open");
        assert_eq!(
            f.params,
            vec![
                "@work".to_owned(),
                "@work".to_owned(),
                "x".to_owned(),
                "open".to_owned()
            ]
        );
        assert_eq!(f.sql.matches('?').count(), f.params.len());
    }

    #[test]
    fn the_identity_expression_compiles_to_true() {
        let f = sql("");
        assert_eq!(f.sql, "1");
        assert!(
            f.exact,
            "matching everything is precise when nothing is asked"
        );
    }
}
