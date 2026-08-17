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

use crate::core::domain::filter_expr::{Atom, CompareOp, Expr, Field, Named, TextField, Value};
use crate::core::domain::task::Priority;

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
            Some(f) => SqlFilter::new(negate(&f.sql), f.params, true),
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
            Some(SqlFilter::new(negate(&f.sql), f.params, true))
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

/// Negates a fragment, treating SQL's `NULL` as false first.
///
/// This is where SQL's three-valued logic and the evaluator's two-valued logic
/// meet. A comparison against an unset column yields `NULL`, and `NOT NULL` is
/// `NULL` — not true — so the row is dropped. The evaluator instead reads an
/// unset field as "the predicate is false", and negating that gives *true*.
///
/// So `not slug:alpha` must return the tasks with no slug at all, and plain
/// `NOT (slug IN (?))` returned none of them. `COALESCE(…, 0)` collapses the
/// unknown to false before the negation, which is exactly the evaluator's rule.
fn negate(sql: &str) -> String {
    format!("(NOT COALESCE({sql}, 0))")
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
/// The descendant half is a `substr` comparison rather than `LIKE ? || '/%'`,
/// and both differences matter because this atom is marked *exact*, so nothing
/// re-checks it in memory:
///
/// - SQLite's `LIKE` folds ASCII case by default, so `+@WORK` would have
///   matched a task tagged `@work/x` that `tag_matches` rejects — extra rows,
///   silently.
/// - `_` is a `LIKE` wildcard *and* a legal tag character, so `+a_b` would
///   have matched `axb/c`.
///
/// `=` on TEXT uses BINARY collation and has no wildcards, so the comparison
/// means exactly what the evaluator means. Binds its value three times.
const TAG_MATCH: &str = "EXISTS (SELECT 1 FROM task_tags tt \
     WHERE tt.task_id = tasks.id \
     AND (tt.tag = ? OR substr(tt.tag, 1, length(?) + 1) = ? || '/'))";

/// The parameters [`TAG_MATCH`] binds, in order.
fn tag_params(name: &str) -> Vec<String> {
    vec![name.to_owned(), name.to_owned(), name.to_owned()]
}

/// Priority is stored as a word, so ordering needs an explicit rank — `'high'`
/// sorts before `'low'` lexicographically, which is the opposite of its urgency.
const PRIORITY_RANK: &str =
    "(CASE priority WHEN 'low' THEN 0 WHEN 'medium' THEN 1 WHEN 'high' THEN 2 END)";

fn atom_sql(atom: &Atom, today: NaiveDate) -> Option<SqlFilter> {
    match atom {
        Atom::Tag(name) => Some(SqlFilter::new(TAG_MATCH, tag_params(name), true)),
        Atom::Equals { field, values } => equals_sql(field, values, today),
        Atom::Compare { field, op, value } => compare_sql(field, *op, value, today),
        Atom::Range { field, low, high } => {
            let lo = compare_sql(field, CompareOp::Ge, low, today)?;
            let hi = compare_sql(field, CompareOp::Le, high, today)?;
            Some(combine(vec![lo, hi], " AND "))
        }
        Atom::Has(field) => has_sql(field),
        Atom::Is(named) => is_sql(*named, today),
        Atom::Search {
            field,
            term,
            prefix,
        } => search_sql(*field, term, *prefix),
    }
}

/// A search atom, as an FTS5 `MATCH` against the `task_fts` index.
///
/// The term is tokenised with the evaluator's own [`words`] and the phrase is
/// rebuilt from those tokens, so what reaches SQLite is a quoted string of
/// alphanumerics. That has two consequences worth stating plainly:
///
/// - **No FTS5 injection is possible.** A user searching for `AND`, `*`, `"`
///   or `^` gets those characters tokenised away or quoted as literals, never
///   interpreted as query syntax — and never a MATCH syntax error either.
/// - **The index answers what the scan would answer**, because both start from
///   the same tokens. That is what lets this atom claim to be exact.
///
/// A term with no word characters at all (`"---"`) matches nothing, exactly as
/// the scan says: there is no token to look for.
fn search_sql(field: Option<TextField>, term: &str, prefix: bool) -> Option<SqlFilter> {
    let tokens = crate::core::domain::filter_eval::words(term);
    if tokens.is_empty() {
        return Some(SqlFilter::nothing());
    }
    // ── The limit of the agreement ──────────────────────────────────────────
    //
    // The scan tokenises with Rust's `is_alphanumeric` + `to_lowercase`; FTS5
    // uses unicode61's own tables. Over ASCII those agree exactly. Beyond it
    // they do not, and not always in the safe direction:
    //
    // - `is_alphanumeric` keeps combining marks (NFD accents, Indic vowel
    //   signs) inside a token where unicode61 treats them as separators;
    // - `to_lowercase` folds cases unicode61 does not (`İ` becomes two chars,
    //   Cherokee, Georgian Mtavruli).
    //
    // Some of those make FTS match MORE than the scan, which a re-check could
    // absorb — but others make it match LESS, and a subset silently drops rows
    // no matter what the caller does with `exact`. Rather than guess which,
    // anything outside ASCII is left to the scan: correct always, slower for a
    // rare query. Pinned by `tests/sqlite_assumptions.rs`.
    if tokens.iter().any(|t| !t.is_ascii()) {
        return None;
    }
    // Tokens are alphanumeric-only, so the quotes cannot be escaped out of.
    let mut query = format!("\"{}\"", tokens.join(" "));
    if prefix {
        query.push('*');
    }
    if let Some(text) = field {
        // FTS5 column filter. The column name is ours, not the user's.
        query = format!("{{{}}} : {}", text.name(), query);
    }
    // `IN (SELECT …)`, deliberately not `EXISTS (… AND task_id = tasks.id)`.
    // The `EXISTS` form is CORRELATED: SQLite re-runs the MATCH once per task
    // row, which turns a single index lookup into one full-text query per task
    // and cost 2.4 s at 5 000 tasks in the bench. This form runs the MATCH
    // once, materialises the matching ids, and probes `tasks` by primary key.
    Some(SqlFilter::new(
        "tasks.rowid IN (SELECT rowid FROM task_fts WHERE task_fts MATCH ?)",
        vec![query],
        true,
    ))
}

/// `field:a,b` — membership, which is a disjunction over the values.
fn equals_sql(field: &Field, values: &[Value], today: NaiveDate) -> Option<SqlFilter> {
    // Tag equality is the only field whose column lives in another table.
    if matches!(field, Field::Tag | Field::Context) {
        let parts: Vec<SqlFilter> = values
            .iter()
            .map(|v| SqlFilter::new(TAG_MATCH, tag_params(&v.raw), true))
            .collect();
        return Some(combine(parts, " OR "));
    }

    // Enum-valued columns store one canonical spelling, but the evaluator
    // parses the value case-insensitively and accepts aliases (`med`,
    // `canceled`). Binding the raw text into a case-sensitive `IN` therefore
    // DROPPED rows the evaluator matches — and since this atom is exact,
    // nothing re-checked them. Normalise through the same parser instead.
    if let Some(canonical) = canonical_enum(field, values) {
        return Some(canonical);
    }

    if *field == Field::Id {
        let parts: Vec<SqlFilter> = values.iter().map(|v| id_sql(&v.raw)).collect();
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

/// `id:<uuid or prefix>`, matched the way the evaluator matches it.
///
/// The `id` column stores the hyphenated spelling while the evaluator compares
/// the dashless one, so the hyphens come out of the column before the
/// comparison. Two shapes rather than one, because the whole-UUID case is
/// worth keeping fast: it is an equality on the PRIMARY KEY, and a
/// `substr(replace(...))` around the column would throw that index away for
/// the single most common way to write this predicate.
///
/// The prefix length is written into the SQL rather than bound. That is safe
/// for the same reason the priority rank is: the value is not user text but a
/// number derived from a string [`normalize_id`] has already proved to be
/// 4 to 32 hex digits. Binding it would also be wrong — SQLite would hand
/// `substr` a TEXT third argument.
fn id_sql(raw: &str) -> SqlFilter {
    // The parser refuses a malformed id, but `TaskQuery::filter` is public and
    // this compiler does not get to assume a distant guard ran.
    let Some(id) = crate::core::domain::filter_expr::normalize_id(raw) else {
        return SqlFilter::nothing();
    };
    match uuid::Uuid::parse_str(&id) {
        Ok(uuid) => SqlFilter::new("id = ?", vec![uuid.hyphenated().to_string()], true),
        Err(_) => SqlFilter::new(
            format!("substr(replace(id, '-', ''), 1, {}) = ?", id.len()),
            vec![id],
            true,
        ),
    }
}

/// `status:` / `priority:` translated through the evaluator's own parser, so
/// the SQL binds the spelling the column actually stores.
///
/// A value that does not parse matches nothing — the same answer the evaluator
/// gives — so it compiles to `0` rather than to a string that happens to match
/// no row, keeping the two paths identical for garbage input too.
fn canonical_enum(field: &Field, values: &[Value]) -> Option<SqlFilter> {
    let column = match field {
        Field::Status => "status",
        Field::Priority => "priority",
        _ => return None,
    };
    let parts: Vec<SqlFilter> = values
        .iter()
        .map(|v| {
            let canonical = match field {
                Field::Status => {
                    crate::core::domain::filter_eval::status_from(&v.raw).map(|s| s.to_string())
                }
                _ => v.raw.parse::<Priority>().ok().map(|p| p.to_string()),
            };
            match canonical {
                Some(word) => SqlFilter::new(format!("{column} = ?"), vec![word], true),
                None => SqlFilter::nothing(),
            }
        })
        .collect();
    Some(combine(parts, " OR "))
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
    // Every row has one — the column is the PRIMARY KEY — so `has:id` is the
    // whole table and `no:id` is empty, exactly as the evaluator says. Spelled
    // out here because `id` is deliberately absent from `text_column`: sharing
    // that lookup would let `equals_sql` fall through to `id IN (?)`, which is
    // an equality where the evaluator does a prefix match.
    if *field == Field::Id {
        return Some(SqlFilter::new("1", Vec::new(), true));
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
/// correctly as text.
///
/// `created_at` and `updated_at` are deliberately ABSENT even though the cache
/// has both columns. The in-memory reference reads them from the `task_dates`
/// map, and a storage query carries none — so it answers "matches nothing"
/// where SQL would answer from the column. Translating them would make the SQL
/// *exact* and therefore unchecked, and the two paths would disagree.
/// `validate_for_store` refuses these fields at the front door, but it is far
/// from here and `TaskQuery::filter` is public, so the compiler declines to
/// claim an exactness it cannot honour rather than relying on a distant guard.
fn date_column(field: &Field) -> Option<&'static str> {
    Some(match field {
        Field::Due => "due",
        Field::Start => "start",
        Field::Completed => "completed_at",
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
            "has:due",
            "has:assignee",
            "has:tag",
            "no:due",
            "is:closed",
            "is:archived",
            "is:assigned",
            "is:overdue",
            // Search, as of S3 — the FTS index answers what the scan would.
            "sometext",
            "\"a phrase\"",
            "arch*",
            "title:something",
            "notes:measured",
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
            "data.estimate:3", // inside the JSON blob
            "data.estimate>1", // ditto, ordered
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

    // Search used to be the canonical unpushable atom; since S3 it compiles to
    // an FTS MATCH, so these use `data.*` — still residual, because the value
    // lives in the JSON blob.

    #[test]
    fn a_conjunction_keeps_the_half_it_can_push() {
        // The tag narrows in SQL, the data predicate is rechecked in memory.
        let f = sql("+@work data.estimate:3");
        assert!(!f.exact);
        assert!(f.sql.contains("task_tags"), "{}", f.sql);
        assert!(f.sql.contains(" AND 1"), "{}", f.sql);
    }

    #[test]
    fn a_disjunction_with_an_unpushable_branch_widens_to_everything() {
        // THE subset trap: pushing only `+@work` here would drop every task
        // that matched the data predicate but not the tag.
        let f = sql("+@work or data.estimate:3");
        assert!(!f.exact);
        assert!(
            f.sql.starts_with("(EXISTS (SELECT 1 FROM task_tags"),
            "{}",
            f.sql
        );
        assert!(
            f.sql.ends_with(" OR 1)"),
            "the OR must still admit everything the residual could match — {}",
            f.sql
        );
    }

    #[test]
    fn negating_an_inexact_operand_widens_instead_of_narrowing() {
        // `not (residual)` must not become `NOT 1` — that is empty, and every
        // task not matching the term would vanish.
        let f = sql("not data.estimate:3");
        assert!(!f.exact);
        assert_eq!(f.sql, "1");

        // Same trap one level down: the operand mixes pushable and not.
        let f = sql("not (+@work and data.estimate:3)");
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
        // not(a or residual) → widens; and(that, b) → keeps b.
        let f = sql("not (+a or data.estimate:3) and +b");
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
    fn a_whole_id_is_a_primary_key_lookup_and_a_prefix_is_not() {
        // The column stores the hyphenated spelling, so a full id binds that
        // form and stays an equality the PRIMARY KEY index can serve.
        let f = sql("id:a1b2c3d4-e5f6-0718-293a-4b5c6d7e8f90");
        assert_eq!(f.sql, "id = ?");
        assert_eq!(f.params, vec!["a1b2c3d4-e5f6-0718-293a-4b5c6d7e8f90"]);
        assert!(f.exact);

        // Written without hyphens it means the same task, so it compiles to
        // the same lookup rather than to a 32-character prefix test.
        assert_eq!(sql("id:a1b2c3d4e5f60718293a4b5c6d7e8f90").sql, "id = ?");

        // A prefix has to strip the column's hyphens to compare against what
        // the evaluator compares against.
        let f = sql("id:a1b2c3d4");
        assert_eq!(f.sql, "substr(replace(id, '-', ''), 1, 8) = ?");
        assert_eq!(f.params, vec!["a1b2c3d4".to_owned()]);
        assert!(f.exact);

        // Case is folded into the stored spelling, not left to SQLite: `=` on
        // TEXT is BINARY, so an uppercase paste would otherwise match nothing
        // while the evaluator matches — and this atom is exact, so nothing
        // would re-check it.
        assert_eq!(sql("id:A1B2C3D4").params, vec!["a1b2c3d4".to_owned()]);

        // The length is a literal, never a bound parameter: `substr` needs an
        // INTEGER third argument and a bound one arrives as TEXT.
        assert_eq!(sql("id:a1b2c3d4").sql.matches('?').count(), 1);

        // `has:id` is the whole table — the column is the primary key.
        assert_eq!(sql("has:id").sql, "1");
        assert!(sql("has:id").exact);
        assert_eq!(sql("no:id").sql, "(NOT COALESCE(1, 0))");
    }

    #[test]
    fn an_id_that_cannot_name_a_task_matches_nothing() {
        // The parser refuses these, but `TaskQuery::filter` is public and this
        // compiler does not get to assume a distant guard ran.
        use crate::core::domain::filter_expr::{Atom, Field, Value};
        for raw in ["zzzz", "ab", &"a".repeat(33)] {
            let expr = Expr::Atom(Atom::Equals {
                field: Field::Id,
                values: vec![Value::word(raw)],
            });
            let f = compile(&expr, today());
            assert_eq!(f.sql, "0", "{raw}");
            assert!(f.exact, "{raw}: matching nothing is a precise answer");
        }
    }

    #[test]
    fn an_unknown_enum_value_matches_nothing() {
        assert_eq!(sql("priority>sideways").sql, "0");
        assert_eq!(sql("status:sideways").sql, "0");
        assert!(sql("status:sideways").params.is_empty());
    }

    #[test]
    fn enum_values_are_normalised_to_the_stored_spelling() {
        // The evaluator parses these case-insensitively and accepts aliases.
        // Binding the raw text into a case-sensitive comparison DROPPED every
        // matching row, and the atom is exact so nothing re-checked it.
        for (query, want) in [
            ("status:open", "open"),
            ("status:OPEN", "open"),
            ("status:Done", "done"),
            ("status:canceled", "cancelled"),
            ("priority:high", "high"),
            ("priority:HIGH", "high"),
            ("priority:med", "medium"),
        ] {
            let f = sql(query);
            assert_eq!(f.params, vec![want.to_owned()], "{query}");
            assert!(f.exact, "{query}");
        }
    }

    #[test]
    fn the_tag_prefix_test_is_case_sensitive_and_wildcard_free() {
        // `LIKE` folds ASCII case and treats `_` as a wildcard, and `_` is a
        // legal tag character — so the old translation matched tags that
        // `tag_matches` rejects, while claiming to be exact.
        let f = sql("+@work");
        assert!(!f.sql.contains("LIKE"), "{}", f.sql);
        assert!(
            f.sql.contains("substr(tt.tag, 1, length(?) + 1)"),
            "{}",
            f.sql
        );
        assert_eq!(f.params.len(), 3, "the value is bound once per placeholder");
        assert_eq!(f.sql.matches('?').count(), f.params.len());
    }

    #[test]
    fn git_dates_are_not_pushed_even_though_the_columns_exist() {
        // The reference evaluator reads these from the `task_dates` map, and a
        // storage query carries none — so it answers "matches nothing" where
        // SQL would answer from the column. Pushing them would mark the atom
        // exact, nothing would re-check it, and the two paths would disagree.
        for query in ["created>2026-01-01", "updated<today", "has:created"] {
            let f = sql(query);
            assert!(!f.exact, "{query} must not claim exactness");
            assert_eq!(f.sql, "1", "{query}");
        }
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
        assert_eq!(f.params, vec!["@work".to_owned(); 3]);
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

// The SQLite behaviours this compiler depends on — LIKE's case folding, the
// integer/text sort order, NULL under NOT, FTS5 tokenisation — are pinned in
// `tests/sqlite_assumptions.rs`. They are properties of the engine rather than
// of this code, and each one is load-bearing for a predicate marked *exact*.
