//! Evaluating a parsed [`Expr`] against a [`Task`].
//!
//! Stage 1 of the filter language: the grammar from
//! [`filter_expr`](crate::core::domain::filter_expr) becomes a decision about
//! a task. [`eval`] is a pure predicate — it never loads anything — so
//! everything it needs beyond the task itself is precomputed once per query
//! into an [`EvalCtx`].
//!
//! # This is the reference semantics
//!
//! Stage 2 compiles the pushable half of an expression into SQL and stage 3
//! replaces the text scan with an FTS5 index. Both are optimisations that must
//! agree with what this module decides, not the other way round: the
//! differential tests those stages carry use [`eval`] as the oracle. Write
//! changes here as definitions, not as tweaks.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::core::domain::date_parse;
use crate::core::domain::filter_expr::{Atom, CompareOp, Expr, Field, Named, TextField, Value};
use crate::core::domain::tag;
use crate::core::domain::task::{Priority, Status, Task};
use crate::core::scoring::TaskDates;

/// Everything an expression needs about the world beyond the task in hand.
///
/// The indexes are relational — they answer questions about *other* tasks —
/// so they are built once per query by [`EvalIndexes::build`] rather than
/// rediscovered per task.
pub struct EvalCtx<'a> {
    /// The reference date for relative values (`due<+7d`, `is:overdue`).
    pub today: NaiveDate,

    /// Relational indexes over the candidate set.
    pub indexes: &'a EvalIndexes,

    /// Git-derived timestamps, for `created:` and `updated:`.
    ///
    /// A task with no entry — never committed, or a caller that had no dates to
    /// hand — is treated as having those fields unset, so `has:created` is
    /// false for it rather than the query silently matching everything.
    pub task_dates: &'a HashMap<Uuid, TaskDates>,

    /// Whether the tasks being evaluated come from the archived tier, which is
    /// what `is:archived` reports. Archiving is a property of where a task is
    /// stored, not a field on it, so the tier has to be told to the evaluator.
    pub archived: bool,
}

/// Relational facts about a candidate set, precomputed for [`eval`].
#[derive(Debug, Default)]
pub struct EvalIndexes {
    /// IDs of tasks that are open or started — what `is:blocked` consults.
    open_ids: HashSet<Uuid>,

    /// IDs that are some other task's `parent_id`: `is:project`.
    parent_ids: HashSet<Uuid>,

    /// `id -> (parent_id, slug)`, for walking a task's ancestry when resolving
    /// `parent:<slug>`.
    lineage: HashMap<Uuid, (Option<Uuid>, Option<String>)>,
}

impl EvalIndexes {
    /// Builds the indexes `expr` will actually consult, in one pass.
    ///
    /// `lineage` is the expensive one — a map entry and a cloned slug per task
    /// — and it is read only by `parent:`, which the surfaces lift out of the
    /// expression before it ever reaches here. Building it unconditionally
    /// cost every query an allocation per task for a code path no query could
    /// reach, which matters at the scale this repo benchmarks at.
    pub fn build_for(expr: &Expr, tasks: &[Task]) -> Self {
        Self::build_inner(tasks, mentions_parent(expr))
    }

    /// Builds every index, whatever any expression might need — for callers
    /// that have no expression to inspect (tests, and any future programmatic
    /// AST).
    pub fn build(tasks: &[Task]) -> Self {
        Self::build_inner(tasks, true)
    }

    fn build_inner(tasks: &[Task], wants_lineage: bool) -> Self {
        let mut indexes = EvalIndexes {
            open_ids: HashSet::new(),
            parent_ids: HashSet::new(),
            lineage: if wants_lineage {
                HashMap::with_capacity(tasks.len())
            } else {
                HashMap::new()
            },
        };
        for task in tasks {
            if task.is_active() {
                indexes.open_ids.insert(task.id);
            }
            if let Some(parent) = task.parent_id {
                indexes.parent_ids.insert(parent);
            }
            if wants_lineage {
                indexes
                    .lineage
                    .insert(task.id, (task.parent_id, task.slug.clone()));
            }
        }
        indexes
    }

    /// IDs of the open tasks in the candidate set.
    pub fn open_ids(&self) -> &HashSet<Uuid> {
        &self.open_ids
    }

    /// Whether `task` is the task slugged `slug` or one of its descendants —
    /// the project scope `parent:<slug>` selects.
    ///
    /// The walk is bounded by the number of known tasks, so a cycle introduced
    /// by a corrupt store cannot hang the query.
    fn in_project(&self, task: &Task, slug: &str) -> bool {
        if task.slug.as_deref() == Some(slug) {
            return true;
        }
        let mut current = task.parent_id;
        for _ in 0..self.lineage.len() {
            let Some(id) = current else { return false };
            let Some((parent, task_slug)) = self.lineage.get(&id) else {
                return false;
            };
            if task_slug.as_deref() == Some(slug) {
                return true;
            }
            current = *parent;
        }
        false
    }
}

/// Whether `expr` contains a `parent:` predicate, which is the only thing that
/// reads the lineage index.
fn mentions_parent(expr: &Expr) -> bool {
    match expr {
        Expr::And(parts) | Expr::Or(parts) => parts.iter().any(mentions_parent),
        Expr::Not(inner) => mentions_parent(inner),
        Expr::Atom(Atom::Equals { field, .. }) => *field == Field::Parent,
        Expr::Atom(_) => false,
    }
}

/// Whether `task` satisfies `expr`.
///
/// `And(vec![])` — what an empty query parses to — matches everything, so a
/// caller with no filter needs no special case.
pub fn eval(expr: &Expr, task: &Task, ctx: &EvalCtx) -> bool {
    match expr {
        Expr::And(parts) => parts.iter().all(|p| eval(p, task, ctx)),
        Expr::Or(parts) => parts.iter().any(|p| eval(p, task, ctx)),
        Expr::Not(inner) => !eval(inner, task, ctx),
        Expr::Atom(atom) => eval_atom(atom, task, ctx),
    }
}

fn eval_atom(atom: &Atom, task: &Task, ctx: &EvalCtx) -> bool {
    match atom {
        Atom::Search {
            field,
            term,
            prefix,
        } => search(task, *field, term, *prefix),
        Atom::Tag(name) => has_tag(task, name),
        Atom::Equals { field, values } => values.iter().any(|v| equals(field, v, task, ctx)),
        Atom::Compare { field, op, value } => compare(field, *op, value, task, ctx),
        Atom::Range { field, low, high } => {
            compare(field, CompareOp::Ge, low, task, ctx)
                && compare(field, CompareOp::Le, high, task, ctx)
        }
        Atom::Has(field) => has(field, task, ctx),
        Atom::Is(named) => is(*named, task, ctx),
    }
}

// ─── Text search ────────────────────────────────────────────────────────────

/// The text a search reads when no field scopes it.
///
/// `slug` is deliberately absent: it is an identifier, so `slug:x` is an exact
/// match and a bare word never matches a title-derived filename.
const SEARCHED_FIELDS: [TextField; 4] = [
    TextField::Title,
    TextField::Description,
    TextField::Notes,
    TextField::Url,
];

fn text_of(task: &Task, field: TextField) -> Option<&str> {
    match field {
        TextField::Title => Some(task.title.as_str()),
        TextField::Description => task.description.as_deref(),
        TextField::Notes => task.notes.as_deref(),
        TextField::Url => task.url.as_deref(),
    }
}

/// Case-insensitive, word-oriented search.
///
/// Matching is defined on **words**, not on raw substrings, because stage 3
/// backs this atom with an FTS5 index and an index cannot match mid-word. A
/// term is split into words the same way the haystack is, and matches where
/// that sequence of words appears consecutively:
///
/// - `arch` matches "arch" but not "archive" — a bare word is a whole word;
/// - `arch*` matches "archive", because `prefix` relaxes the *last* word;
/// - `"cold tier"` matches "the cold tier segment" but not "tier cold".
///
/// A term with no word characters at all (`"---"`) matches nothing; there is
/// no sequence of words to look for, and an index would have nothing to store.
fn search(task: &Task, field: Option<TextField>, term: &str, prefix: bool) -> bool {
    let needle: Vec<String> = words(term);
    if needle.is_empty() {
        return false;
    }
    let fields: &[TextField] = match &field {
        Some(one) => std::slice::from_ref(one),
        None => &SEARCHED_FIELDS,
    };
    fields
        .iter()
        .filter_map(|f| text_of(task, *f))
        .any(|text| contains_words(&words(text), &needle, prefix))
}

/// Splits text into lowercase alphanumeric words, which is the tokenisation an
/// FTS5 `unicode61` index performs.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// Whether `needle` appears as a consecutive run inside `haystack`, with the
/// final needle word matched by prefix when `prefix` is set.
fn contains_words(haystack: &[String], needle: &[String], prefix: bool) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    let last = needle.len() - 1;
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .enumerate()
            .all(|(i, (word, want))| {
                if prefix && i == last {
                    word.starts_with(want)
                } else {
                    word == want
                }
            })
    })
}

// ─── Tags ───────────────────────────────────────────────────────────────────

/// Whether the task carries `name` or a descendant of it. One rule for every
/// kind of tag — `@`, `#` and freeform are conventions, not behaviour.
fn has_tag(task: &Task, name: &str) -> bool {
    task.tags.iter().any(|t| tag::tag_matches(name, t))
}

// ─── Equality ───────────────────────────────────────────────────────────────

fn equals(field: &Field, value: &Value, task: &Task, ctx: &EvalCtx) -> bool {
    match field {
        Field::Status => status_from(&value.raw).is_some_and(|s| task.status == s),
        Field::Priority => value
            .raw
            .parse::<Priority>()
            .is_ok_and(|p| task.priority == p),
        Field::Due | Field::Start | Field::Completed | Field::Created | Field::Updated => {
            match (date_of(field, task, ctx), resolve_date(value, ctx.today)) {
                (Some(actual), Some(wanted)) => actual == wanted,
                _ => false,
            }
        }
        Field::Assignee => task.assignee.as_deref() == Some(value.raw.as_str()),
        Field::Slug => task.slug.as_deref() == Some(value.raw.as_str()),
        Field::Parent => ctx.indexes.in_project(task, &value.raw),
        // `tag:` and `context:` are the long spellings of a tag test; the
        // parser already rewrites `tag:` to `Atom::Tag`, and `context:` differs
        // only in naming the convention it expects.
        Field::Tag | Field::Context => has_tag(task, &value.raw),
        // `user:` keeps the meaning it has as a view filter: an unassigned task
        // belongs to whoever is looking at it.
        Field::User => task
            .assignee
            .as_ref()
            .is_none_or(|a| a == value.raw.as_str()),
        Field::Title | Field::Description | Field::Notes | Field::Url => {
            let text = field.as_text().expect("text field");
            search(task, Some(text), &value.raw, false)
        }
        Field::Score => false,
        Field::Data(key) => task
            .data
            .get(key)
            .is_some_and(|actual| json_equals(actual, &value.raw)),
    }
}

/// `Status` has no `FromStr`; filters need one and nothing else does.
///
/// Public because the SQL compiler must accept exactly the spellings this
/// accepts. Two parsers would mean `status:OPEN` matching here and not there.
pub fn status_from(raw: &str) -> Option<Status> {
    match raw.to_ascii_lowercase().as_str() {
        "open" => Some(Status::Open),
        "started" => Some(Status::Started),
        "done" => Some(Status::Done),
        "cancelled" | "canceled" => Some(Status::Cancelled),
        _ => None,
    }
}

/// Compares a task data entry against the value as written.
///
/// Data values are arbitrary JSON, so the comparison follows the stored type:
/// a string compares as text, a number numerically (`1` matches `1.0`), a bool
/// against `true`/`false`. An array or object never compares equal — a filter
/// value is one scalar, and pretending it could stand for a whole structure
/// would make `data.k:x` quietly wrong rather than obviously unsupported. Use
/// `has:data.<key>` to test for one.
fn json_equals(actual: &JsonValue, raw: &str) -> bool {
    match actual {
        JsonValue::String(s) => s == raw,
        JsonValue::Number(n) => match (n.as_f64(), raw.parse::<f64>()) {
            (Some(a), Ok(b)) => a == b,
            _ => false,
        },
        JsonValue::Bool(b) => raw.parse::<bool>().is_ok_and(|want| *b == want),
        JsonValue::Null => raw == "null",
        JsonValue::Array(_) | JsonValue::Object(_) => false,
    }
}

// ─── Ordered comparison ─────────────────────────────────────────────────────

/// `field <op> value`, for the fields the parser admits an operator on.
///
/// A field that is unset on this task never compares true: "no due date" is
/// not earlier than a deadline, it is absent, and treating it as either
/// extreme would make `due<x` and `not due>=x` disagree.
fn compare(field: &Field, op: CompareOp, value: &Value, task: &Task, ctx: &EvalCtx) -> bool {
    match field {
        Field::Priority => match value.raw.parse::<Priority>() {
            Ok(wanted) => ordered(priority_rank(&task.priority), priority_rank(&wanted), op),
            Err(_) => false,
        },
        Field::Due | Field::Start | Field::Completed | Field::Created | Field::Updated => {
            match (date_of(field, task, ctx), resolve_date(value, ctx.today)) {
                (Some(actual), Some(wanted)) => ordered(actual, wanted, op),
                _ => false,
            }
        }
        Field::Data(key) => match task.data.get(key) {
            Some(actual) => json_ordered(actual, &value.raw, op),
            None => false,
        },
        // Scoring runs after filtering over a parent-extended pool, so no score
        // exists yet at this point. `validate` rejects the field before a query
        // can reach here.
        Field::Score => false,
        _ => false,
    }
}

fn ordered<T: PartialOrd>(actual: T, wanted: T, op: CompareOp) -> bool {
    match op {
        CompareOp::Lt => actual < wanted,
        CompareOp::Le => actual <= wanted,
        CompareOp::Gt => actual > wanted,
        CompareOp::Ge => actual >= wanted,
    }
}

/// Low < Medium < High, so `priority>medium` means "more urgent than medium".
fn priority_rank(priority: &Priority) -> u8 {
    match priority {
        Priority::Low => 0,
        Priority::Medium => 1,
        Priority::High => 2,
    }
}

/// Ordered comparison over a data entry: numeric when both sides are numbers,
/// otherwise lexicographic over the text form.
fn json_ordered(actual: &JsonValue, raw: &str, op: CompareOp) -> bool {
    if let (Some(a), Ok(b)) = (actual.as_f64(), raw.parse::<f64>()) {
        return ordered(a, b, op);
    }
    match actual {
        JsonValue::String(s) => ordered(s.as_str(), raw, op),
        _ => false,
    }
}

// ─── Dates ──────────────────────────────────────────────────────────────────

fn date_of(field: &Field, task: &Task, ctx: &EvalCtx) -> Option<NaiveDate> {
    match field {
        Field::Due => task.due,
        Field::Start => task.start,
        Field::Completed => task.completed_at,
        Field::Created => ctx
            .task_dates
            .get(&task.id)
            .map(|d| d.created_at.date_naive()),
        Field::Updated => ctx
            .task_dates
            .get(&task.id)
            .map(|d| d.updated_at.date_naive()),
        _ => None,
    }
}

/// Resolves a date value written in a filter.
///
/// The quoting decides the dialect, exactly as the grammar promises: a bare
/// value takes the compact forms (`2026-08-10`, `+7d`, `eow`), a quoted one is
/// handed to `interim` for natural language (`"next monday"`). Resolution
/// happens here rather than at parse time because it needs the query's `today`.
///
/// Public because the SQL compiler must resolve a value to the *same* date
/// this evaluator would; two resolvers would be two dialects.
pub fn resolve_date(value: &Value, today: NaiveDate) -> Option<NaiveDate> {
    if let Some(date) = date_parse::parse_compact(&value.raw, today) {
        return Some(date);
    }
    if value.quoted {
        return date_parse::parse_date(&value.raw, today).ok();
    }
    None
}

// ─── has: and is: ───────────────────────────────────────────────────────────

/// Whether the field is set on this task.
///
/// `has:context` is how the `@` convention stays queryable now that a context
/// is an ordinary tag: it asks whether the task is filed under any context at
/// all, which is what an active context used to imply.
fn has(field: &Field, task: &Task, ctx: &EvalCtx) -> bool {
    match field {
        // Always present on every task.
        Field::Title | Field::Status | Field::Priority => true,
        Field::Due => task.due.is_some(),
        Field::Start => task.start.is_some(),
        Field::Completed => task.completed_at.is_some(),
        Field::Created | Field::Updated => ctx.task_dates.contains_key(&task.id),
        Field::Assignee | Field::User => task.assignee.is_some(),
        Field::Slug => task.slug.is_some(),
        Field::Parent => task.parent_id.is_some(),
        Field::Tag => !task.tags.is_empty(),
        Field::Context => task.tags.iter().any(|t| tag::is_context(t)),
        Field::Description => task.description.is_some(),
        Field::Notes => task.notes.is_some(),
        Field::Url => task.url.is_some(),
        Field::Score => false,
        Field::Data(key) => task.data.contains_key(key),
    }
}

fn is(named: Named, task: &Task, ctx: &EvalCtx) -> bool {
    match named {
        // A closed task with a deadline in the past is late, not outstanding.
        Named::Overdue => task.is_active() && task.due.is_some_and(|d| d < ctx.today),
        Named::Blocked => task
            .blocked_by
            .iter()
            .any(|id| ctx.indexes.open_ids.contains(id)),
        // Having children at all, whatever their status — the gate's narrower
        // "parent with *open* children" is about what to work on next, which is
        // a different question from what a project is.
        Named::Project => ctx.indexes.parent_ids.contains(&task.id),
        Named::Recurring => task.recurrence.is_some(),
        Named::Archived => ctx.archived,
        Named::Closed => matches!(task.status, Status::Done | Status::Cancelled),
        Named::Assigned => task.assignee.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::filter_expr::parse;
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
    }

    /// Evaluates `query` against `task`, with `pool` supplying the relational
    /// context (blockers, children, project slugs).
    fn matches_in(query: &str, task: &Task, pool: &[Task]) -> bool {
        let expr = parse(query).unwrap_or_else(|e| panic!("{query}: {e}"));
        let indexes = EvalIndexes::build(pool);
        let ctx = EvalCtx {
            today: today(),
            indexes: &indexes,
            task_dates: &HashMap::new(),
            archived: false,
        };
        eval(&expr, task, &ctx)
    }

    fn matches(query: &str, task: &Task) -> bool {
        matches_in(query, task, std::slice::from_ref(task))
    }

    /// A task exercising every field a filter can name.
    fn sample() -> Task {
        let mut task = Task::new("Rebuild the cold tier cache");
        task.slug = Some("rebuild-cache".into());
        task.priority = Priority::High;
        task.due = Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap());
        task.start = Some(NaiveDate::from_ymd_opt(2026, 5, 10).unwrap());
        task.assignee = Some("alice".into());
        task.tags = vec!["@work/backend".into(), "#printer".into(), "bug".into()];
        task.description = Some("The SQLite cache drifts after a prune.".into());
        task.notes = Some("Measured on the real repository.".into());
        task.url = Some("https://example.invalid/issue/12".into());
        task.data.insert("estimate".into(), json!(3));
        task.data.insert("owner".into(), json!("platform"));
        task.data.insert("urgent".into(), json!(true));
        task
    }

    // ── The identity ─────────────────────────────────────────────────────────

    #[test]
    fn the_empty_expression_matches_everything() {
        assert!(matches("", &Task::new("anything")));
        assert!(matches("", &sample()));
    }

    // ── The bare-token flip ──────────────────────────────────────────────────

    #[test]
    fn a_bare_token_searches_text_and_is_not_a_tag() {
        let mut task = Task::new("nothing to see");
        task.tags = vec!["bug".into()];
        assert!(
            !matches("bug", &task),
            "a bare word must not match a tag of the same name"
        );

        let mut task = Task::new("a bug in the parser");
        assert!(matches("bug", &task), "a bare word searches the text");
        task.tags = vec!["bug".into()];
        assert!(matches("bug", &task));
    }

    #[test]
    fn a_signed_token_matches_the_tag_and_not_the_text() {
        let mut task = Task::new("a bug in the parser");
        assert!(!matches("+bug", &task), "+bug must not match prose");
        task.tags = vec!["bug".into()];
        assert!(matches("+bug", &task));
        assert!(!matches("-bug", &task));
    }

    // ── Search ───────────────────────────────────────────────────────────────

    #[test]
    fn search_covers_title_description_notes_and_url() {
        let task = sample();
        assert!(matches("rebuild", &task), "title");
        assert!(matches("drifts", &task), "description");
        assert!(matches("repository", &task), "notes");
        assert!(matches("issue", &task), "url");
    }

    #[test]
    fn search_does_not_read_the_slug() {
        let mut task = Task::new("unrelated");
        task.slug = Some("water-plants".into());
        assert!(
            !matches("plants", &task),
            "a slug is an identifier, not searchable prose"
        );
    }

    #[test]
    fn search_is_case_insensitive_and_word_bounded() {
        let task = sample();
        assert!(matches("REBUILD", &task));
        assert!(matches("Cold", &task));
        assert!(
            !matches("ebuild", &task),
            "a bare word matches a whole word, never mid-word"
        );
        assert!(
            !matches("rebuil", &task),
            "a bare word is not an implicit prefix"
        );
    }

    #[test]
    fn a_star_makes_the_last_word_a_prefix() {
        let task = sample();
        assert!(matches("rebuil*", &task));
        assert!(matches("cach*", &task));
        assert!(!matches("zcach*", &task));
        assert!(
            matches("\"cold ti\"*", &task),
            "a phrase relaxes only its last word"
        );
        assert!(!matches("\"cld tier\"*", &task));
    }

    #[test]
    fn a_quoted_phrase_matches_words_in_order() {
        let task = sample();
        assert!(matches("\"cold tier\"", &task));
        assert!(
            !matches("\"tier cold\"", &task),
            "a phrase is ordered, not a set"
        );
        assert!(
            !matches("\"rebuild cache\"", &task),
            "a phrase is consecutive, not merely co-occurring"
        );
    }

    #[test]
    fn a_field_scoped_search_reads_only_that_field() {
        let task = sample();
        assert!(matches("title:rebuild", &task));
        assert!(!matches("notes:rebuild", &task));
        assert!(matches("description:sqlite", &task));
        assert!(matches("url:example", &task));
    }

    #[test]
    fn a_term_with_no_words_matches_nothing() {
        let mut task = Task::new("--- ---");
        task.notes = Some("---".into());
        assert!(!matches("\"---\"", &task));
    }

    // ── Tags ─────────────────────────────────────────────────────────────────

    #[test]
    fn a_tag_matches_its_descendants_but_not_its_ancestors() {
        let task = sample(); // carries @work/backend
        assert!(matches("+@work", &task));
        assert!(matches("+@work/backend", &task));
        assert!(!matches("+@work/frontend", &task));

        let mut general = Task::new("general");
        general.tags = vec!["@work".into()];
        assert!(!matches("+@work/backend", &general));
    }

    #[test]
    fn every_kind_of_tag_is_matched_the_same_way() {
        let task = sample();
        for token in ["+@work", "+#printer", "+bug"] {
            assert!(matches(token, &task), "{token}");
        }
    }

    #[test]
    fn tag_colon_is_the_long_spelling_of_plus() {
        let task = sample();
        assert!(matches("tag:@work", &task));
        assert!(matches("tag:@work,@home", &task), "a comma set is an or");
        assert!(!matches("tag:@home", &task));
    }

    // ── Field equality ───────────────────────────────────────────────────────

    #[test]
    fn status_and_priority_equality() {
        let mut task = sample();
        assert!(matches("status:open", &task));
        assert!(!matches("status:done", &task));
        assert!(matches("status:open,done", &task));
        assert!(matches("priority:high", &task));
        assert!(!matches("priority:low", &task));

        task.mark_done(today());
        assert!(matches("status:done", &task));
        task.mark_cancelled();
        assert!(matches("status:cancelled", &task));
    }

    #[test]
    fn an_unknown_enum_value_matches_nothing_rather_than_everything() {
        let task = sample();
        assert!(!matches("status:sideways", &task));
        assert!(!matches("priority:urgent", &task));
    }

    #[test]
    fn slug_is_an_exact_match_not_a_search() {
        let mut task = Task::new("Water the plants");
        task.slug = Some("water-plants".into());
        assert!(matches("slug:water-plants", &task));
        assert!(
            !matches("slug:water", &task),
            "slug:water must not match water-plants"
        );
    }

    #[test]
    fn assignee_equality() {
        let task = sample();
        assert!(matches("assignee:alice", &task));
        assert!(!matches("assignee:bob", &task));
        assert!(matches("assignee:alice,bob", &task));
    }

    #[test]
    fn user_keeps_its_view_meaning_unassigned_belongs_to_everyone() {
        let task = sample(); // assigned to alice
        assert!(matches("user:alice", &task));
        assert!(!matches("user:bob", &task));

        let unassigned = Task::new("shared");
        assert!(
            matches("user:bob", &unassigned),
            "an unassigned task is visible to whoever is looking"
        );
    }

    // ── Dates ────────────────────────────────────────────────────────────────

    #[test]
    fn dates_resolve_against_the_querys_today() {
        let task = sample(); // due 2026-05-20, today is 2026-05-17
        assert!(matches("due:2026-05-20", &task));
        assert!(matches("due:+3d", &task));
        assert!(matches("due<+7d", &task));
        assert!(!matches("due<+1d", &task));
        assert!(matches("due>today", &task));
        assert!(matches("due:2026-05-18..2026-05-25", &task));
        assert!(!matches("due:2026-05-21..2026-05-25", &task));
    }

    #[test]
    fn a_quoted_value_takes_natural_language() {
        let mut task = Task::new("dated");
        task.due = Some(NaiveDate::from_ymd_opt(2026, 5, 18).unwrap());
        assert!(matches("due:\"tomorrow\"", &task));
    }

    #[test]
    fn an_unset_date_never_compares_true() {
        let task = Task::new("no dates");
        for query in ["due:today", "due<+7d", "due>+7d", "due:2026-01-01..eoy"] {
            assert!(!matches(query, &task), "{query}");
        }
        assert!(
            matches("not due<+7d", &task),
            "negation of a false predicate still holds"
        );
    }

    #[test]
    fn created_and_updated_come_from_the_git_dates() {
        let task = sample();
        let expr = parse("created:2026-05-01 and updated>+0d").unwrap();
        let indexes = EvalIndexes::build(std::slice::from_ref(&task));
        let mut dates = HashMap::new();
        dates.insert(
            task.id,
            TaskDates {
                created_at: Utc.with_ymd_and_hms(2026, 5, 1, 9, 0, 0).unwrap(),
                updated_at: Utc.with_ymd_and_hms(2026, 5, 18, 9, 0, 0).unwrap(),
            },
        );
        let ctx = EvalCtx {
            today: today(),
            indexes: &indexes,
            task_dates: &dates,
            archived: false,
        };
        assert!(eval(&expr, &task, &ctx));
        assert!(eval(&parse("has:created").unwrap(), &task, &ctx));

        // Without dates the fields read as unset, not as matching everything.
        let ctx = EvalCtx {
            task_dates: &HashMap::new(),
            ..ctx
        };
        assert!(!eval(&expr, &task, &ctx));
        assert!(!eval(&parse("has:created").unwrap(), &task, &ctx));
    }

    // ── has: / no: ───────────────────────────────────────────────────────────

    #[test]
    fn has_reports_whether_a_field_is_set() {
        let full = sample();
        for query in [
            "has:due",
            "has:start",
            "has:assignee",
            "has:slug",
            "has:tag",
            "has:description",
            "has:notes",
            "has:url",
            "has:title",
        ] {
            assert!(matches(query, &full), "{query}");
        }

        let empty = Task::new("bare");
        for query in [
            "has:due",
            "has:start",
            "has:assignee",
            "has:slug",
            "has:tag",
            "has:description",
            "has:notes",
            "has:url",
            "has:parent",
            "has:completed",
        ] {
            assert!(!matches(query, &empty), "{query}");
            assert!(matches(&query.replace("has:", "no:"), &empty), "{query}");
        }
    }

    #[test]
    fn has_context_asks_whether_the_task_is_filed_under_any_context() {
        let mut task = Task::new("filed");
        task.tags = vec!["bug".into(), "#printer".into()];
        assert!(!matches("has:context", &task));
        assert!(matches("no:context", &task));

        task.tags.push("@work/backend".into());
        assert!(matches("has:context", &task));
    }

    #[test]
    fn has_and_no_over_a_data_key() {
        let task = sample();
        assert!(matches("has:data.estimate", &task));
        assert!(matches("no:data.missing", &task));
        assert!(!matches("has:data.missing", &task));
    }

    // ── Task data ────────────────────────────────────────────────────────────

    #[test]
    fn data_values_compare_by_their_stored_type() {
        let task = sample();
        assert!(matches("data.estimate:3", &task));
        assert!(
            matches("data.estimate:3.0", &task),
            "numbers compare as numbers"
        );
        assert!(!matches("data.estimate:4", &task));
        assert!(matches("data.owner:platform", &task));
        assert!(!matches("data.owner:3", &task));
        assert!(matches("data.urgent:true", &task));
        assert!(!matches("data.urgent:false", &task));
    }

    #[test]
    fn a_structured_data_value_is_only_testable_with_has() {
        let mut task = sample();
        task.data.insert("labels".into(), json!(["a", "b"]));
        assert!(matches("has:data.labels", &task));
        assert!(!matches("data.labels:a", &task));
    }

    #[test]
    fn data_values_can_be_ordered() {
        let task = sample();
        assert!(matches("data.estimate>2", &task));
        assert!(matches("data.estimate<=3", &task));
        assert!(!matches("data.estimate>5", &task));
        assert!(
            !matches("data.missing>0", &task),
            "an absent key never compares true"
        );
    }

    // ── is: ──────────────────────────────────────────────────────────────────

    #[test]
    fn is_overdue_needs_a_past_deadline_on_an_open_task() {
        let mut task = Task::new("late");
        assert!(!matches("is:overdue", &task), "no deadline is not overdue");
        task.due = Some(today() - chrono::Duration::days(1));
        assert!(matches("is:overdue", &task));
        task.due = Some(today());
        assert!(!matches("is:overdue", &task), "due today is not yet late");

        task.due = Some(today() - chrono::Duration::days(1));
        task.mark_done(today());
        assert!(
            !matches("is:overdue", &task),
            "a finished task is not outstanding"
        );
    }

    #[test]
    fn is_blocked_looks_at_whether_the_blocker_is_still_open() {
        let mut blocker = Task::new("blocker");
        let mut blocked = Task::new("blocked");
        blocked.blocked_by = vec![blocker.id];

        let pool = vec![blocker.clone(), blocked.clone()];
        assert!(matches_in("is:blocked", &blocked, &pool));
        assert!(!matches_in("is:blocked", &blocker, &pool));

        blocker.mark_done(today());
        let pool = vec![blocker, blocked.clone()];
        assert!(
            !matches_in("is:blocked", &blocked, &pool),
            "a closed blocker no longer blocks"
        );
    }

    #[test]
    fn is_project_means_the_task_has_children() {
        let parent = Task::new("project");
        let mut child = Task::new("subtask");
        child.parent_id = Some(parent.id);
        let pool = vec![parent.clone(), child.clone()];

        assert!(matches_in("is:project", &parent, &pool));
        assert!(!matches_in("is:project", &child, &pool));
        assert!(matches_in("has:parent", &child, &pool));
    }

    #[test]
    fn is_project_counts_closed_children_too() {
        let parent = Task::new("project");
        let mut child = Task::new("subtask");
        child.parent_id = Some(parent.id);
        child.mark_done(today());
        let pool = vec![parent.clone(), child];
        assert!(matches_in("is:project", &parent, &pool));
    }

    #[test]
    fn is_recurring_closed_and_assigned() {
        let mut task = sample();
        assert!(!matches("is:recurring", &task));
        assert!(!matches("is:closed", &task));
        assert!(matches("is:assigned", &task));

        task.recurrence = Some(crate::core::domain::task::Recurrence::Completion {
            interval_days: 7,
            snap: None,
        });
        assert!(matches("is:recurring", &task));

        task.mark_done(today());
        assert!(matches("is:closed", &task));
        task.mark_cancelled();
        assert!(matches("is:closed", &task));
    }

    #[test]
    fn is_archived_reports_the_tier_being_queried() {
        let task = sample();
        let indexes = EvalIndexes::build(std::slice::from_ref(&task));
        let expr = parse("is:archived").unwrap();
        for archived in [false, true] {
            let ctx = EvalCtx {
                today: today(),
                indexes: &indexes,
                task_dates: &HashMap::new(),
                archived,
            };
            assert_eq!(eval(&expr, &task, &ctx), archived);
        }
    }

    // ── parent: project scope ────────────────────────────────────────────────

    #[test]
    fn parent_selects_the_root_and_every_descendant() {
        let mut root = Task::new("root");
        root.slug = Some("infra".into());
        let mut child = Task::new("child");
        child.parent_id = Some(root.id);
        let mut grandchild = Task::new("grandchild");
        grandchild.parent_id = Some(child.id);
        let outsider = Task::new("outsider");

        let pool = vec![
            root.clone(),
            child.clone(),
            grandchild.clone(),
            outsider.clone(),
        ];
        assert!(matches_in("parent:infra", &root, &pool), "the root itself");
        assert!(matches_in("parent:infra", &child, &pool));
        assert!(matches_in("parent:infra", &grandchild, &pool));
        assert!(!matches_in("parent:infra", &outsider, &pool));
        assert!(!matches_in("parent:nosuch", &child, &pool));
    }

    #[test]
    fn a_parent_cycle_terminates() {
        // A corrupt store must not hang the query.
        let mut a = Task::new("a");
        let mut b = Task::new("b");
        a.parent_id = Some(b.id);
        b.parent_id = Some(a.id);
        let pool = vec![a.clone(), b.clone()];
        assert!(!matches_in("parent:nosuch", &a, &pool));
    }

    // ── Booleans ─────────────────────────────────────────────────────────────

    #[test]
    fn both_operator_spellings_agree() {
        let task = sample();
        for (words, symbols) in [
            ("+bug and priority:high", "+bug & priority:high"),
            ("+bug or priority:low", "+bug | priority:low"),
            ("not +missing", "!+missing"),
        ] {
            assert_eq!(
                matches(words, &task),
                matches(symbols, &task),
                "{words} vs {symbols}"
            );
            assert!(matches(words, &task), "{words}");
        }
    }

    #[test]
    fn adjacency_is_and() {
        let task = sample();
        assert!(matches("+bug priority:high", &task));
        assert!(!matches("+bug priority:low", &task));
    }

    #[test]
    fn precedence_is_not_then_and_then_or() {
        let task = sample(); // +bug, priority:high, assignee alice
                             // `a or b c` is `a or (b and c)`, so the false left branch does not
                             // rescue the conjunction.
        assert!(!matches("+missing or +bug assignee:bob", &task));
        assert!(matches("+missing or +bug assignee:alice", &task));
        assert!(
            matches("(+missing or +bug) assignee:alice", &task),
            "parentheses override precedence"
        );
        assert!(
            matches("not +missing and +bug", &task),
            "not binds tightest"
        );
    }

    #[test]
    fn negation_composes_with_groups() {
        let task = sample();
        assert!(!matches("not (+bug or +missing)", &task));
        assert!(matches("not (+missing or +nothere)", &task));
    }
}
