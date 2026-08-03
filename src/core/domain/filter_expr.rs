//! The filter expression grammar: a query string parsed into an [`Expr`] tree.
//!
//! This is stage 0 of the filter language: parsing only. Nothing here evaluates
//! an expression or touches a [`Task`](crate::core::domain::task::Task) — the
//! evaluator, the SQL pushdown and the surface wiring are separate stages, and
//! the AST is the contract between them.
//!
//! The one design decision that shapes everything else is that **a bare word is
//! a full-text search**, not a tag. Tags therefore require a sigil (`+tag` /
//! `-tag`), which is the form the docs and the MCP examples already use. The
//! three boolean words are reserved so that `a or b` cannot be read as three
//! search terms; quoting (`"or"`) recovers the literal word.
//!
//! Values (dates, durations, numbers) are kept **raw**: resolving `+7d` or
//! `"next monday"` needs a `today` reference the parser has no business owning,
//! so it belongs to the evaluator.

use std::fmt;

use nom::branch::alt;
use nom::bytes::complete::{tag as nom_tag, tag_no_case, take_while};
use nom::character::complete::{char as nom_char, multispace0, satisfy};
use nom::combinator::{all_consuming, cut, opt, recognize};
use nom::error::{ErrorKind, ParseError};
use nom::multi::{many0, separated_list1};
use nom::sequence::{preceded, terminated};
use nom::{IResult, Parser};
use serde::{Deserialize, Serialize};

use crate::core::domain::tag;
use crate::core::error::{Result, TaskError};

// ─── AST ────────────────────────────────────────────────────────────────────

/// A parsed filter expression.
///
/// `And(vec![])` is the identity: it matches every task. An empty query string
/// parses to it, so callers never need a separate "no filter" representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Atom(Atom),
}

/// A single indivisible predicate.
///
/// Surface forms that are pure sugar are desugared here rather than given their
/// own variant, so the evaluator has one case to implement per real predicate:
/// `-tag` becomes `Not(Tag)`, `no:field` becomes `Not(Has)`, and a
/// comma-separated set on a tag or text field becomes an `Or`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Atom {
    /// `word`, `"two words"`, `word*`, `title:word` — full-text search.
    /// `field` is `None` for the default search over every text field.
    Search {
        field: Option<TextField>,
        term: String,
        /// `word*` — match by prefix. The prefix is recorded, never expanded.
        prefix: bool,
    },
    /// `+tag` and `tag:name` — carries this tag or a descendant of it.
    Tag(String),
    /// `field:value` — equality, or membership when comma-separated.
    Equals { field: Field, values: Vec<Value> },
    /// `field<v`, `field<=v`, `field>v`, `field>=v`.
    Compare {
        field: Field,
        op: CompareOp,
        value: Value,
    },
    /// `field:low..high` — inclusive on both ends.
    Range {
        field: Field,
        low: Value,
        high: Value,
    },
    /// `has:field` — the field is set.
    Has(Field),
    /// `is:name` — a named predicate.
    Is(Named),
}

/// A field a predicate can be written against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Status,
    Priority,
    Due,
    Start,
    Completed,
    Created,
    Updated,
    Assignee,
    Slug,
    Parent,
    Tag,
    Context,
    User,
    Title,
    Description,
    Notes,
    Url,
    Score,
    /// `data.<key>` — an entry of the task's free-form data map.
    Data(String),
}

/// The subset of fields a search can be scoped to.
///
/// `slug` is deliberately absent: it is an identifier, not prose. `slug:x`
/// tests equality, so referring to a task by slug in a filter means the same
/// thing as everywhere else in the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextField {
    Title,
    Description,
    Notes,
    Url,
}

/// The ordered comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

/// The `is:` predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Named {
    Overdue,
    Blocked,
    Project,
    Recurring,
    Archived,
    Closed,
    Assigned,
}

/// A predicate's right-hand side, exactly as written.
///
/// `quoted` is kept because it selects the resolver: a bare value uses the
/// compact forms (`2026-08-10`, `+7d`, `today`), while a quoted one is handed
/// to [`date_parse`](crate::core::domain::date_parse) for natural language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub raw: String,
    pub quoted: bool,
}

impl Value {
    /// A bare value, as typed without quotes.
    pub fn word(raw: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            quoted: false,
        }
    }

    /// A double-quoted value.
    pub fn quoted(raw: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            quoted: true,
        }
    }
}

impl Field {
    /// The name this field is written with, which is also how it is printed.
    pub fn name(&self) -> String {
        let fixed = match self {
            Field::Status => "status",
            Field::Priority => "priority",
            Field::Due => "due",
            Field::Start => "start",
            Field::Completed => "completed",
            Field::Created => "created",
            Field::Updated => "updated",
            Field::Assignee => "assignee",
            Field::Slug => "slug",
            Field::Parent => "parent",
            Field::Tag => "tag",
            Field::Context => "context",
            Field::User => "user",
            Field::Title => "title",
            Field::Description => "description",
            Field::Notes => "notes",
            Field::Url => "url",
            Field::Score => "score",
            Field::Data(key) => return format!("data.{key}"),
        };
        fixed.to_owned()
    }

    /// The text field this one scopes a search to, when `field:value` means
    /// "search within this field" rather than "this field equals value".
    pub fn as_text(&self) -> Option<TextField> {
        match self {
            Field::Title => Some(TextField::Title),
            Field::Description => Some(TextField::Description),
            Field::Notes => Some(TextField::Notes),
            Field::Url => Some(TextField::Url),
            _ => None,
        }
    }

    /// Whether `<`, `<=`, `>`, `>=` and `low..high` are meaningful here.
    /// Everything else only takes `:`.
    pub fn is_ordered(&self) -> bool {
        matches!(
            self,
            Field::Priority
                | Field::Due
                | Field::Start
                | Field::Completed
                | Field::Created
                | Field::Updated
                | Field::Score
                | Field::Data(_)
        )
    }
}

impl TextField {
    /// The name this field is written with.
    pub fn name(&self) -> &'static str {
        match self {
            TextField::Title => "title",
            TextField::Description => "description",
            TextField::Notes => "notes",
            TextField::Url => "url",
        }
    }
}

impl CompareOp {
    /// The operator as written.
    pub fn symbol(&self) -> &'static str {
        match self {
            CompareOp::Lt => "<",
            CompareOp::Le => "<=",
            CompareOp::Gt => ">",
            CompareOp::Ge => ">=",
        }
    }
}

impl Named {
    /// Every predicate name, in the order they are listed to the user.
    pub const ALL: [(&'static str, Named); 7] = [
        ("overdue", Named::Overdue),
        ("blocked", Named::Blocked),
        ("project", Named::Project),
        ("recurring", Named::Recurring),
        ("archived", Named::Archived),
        ("closed", Named::Closed),
        ("assigned", Named::Assigned),
    ];

    /// The name this predicate is written with.
    pub fn name(&self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, p)| p == self)
            .map(|(n, _)| *n)
            .unwrap_or("unknown")
    }
}

// ─── Entry point ────────────────────────────────────────────────────────────

/// Parses a filter expression.
///
/// Surfaces join their argv tokens with a space and pass the result here, so
/// `next list +@work -bug` and `next list '+@work -bug'` are the same query.
/// An empty or whitespace-only input yields the match-everything expression.
pub fn parse(input: &str) -> Result<Expr> {
    if input.trim().is_empty() {
        return Ok(Expr::And(Vec::new()));
    }
    match all_consuming(terminated(|i| expr(i, 0), multispace0)).parse(input) {
        Ok((_, parsed)) => Ok(parsed),
        Err(nom::Err::Error(e) | nom::Err::Failure(e)) => Err(TaskError::Other(e.render(input))),
        // Every parser used here is from nom's `complete` family, so a partial
        // parse is reported as an error rather than as `Incomplete`.
        Err(nom::Err::Incomplete(_)) => Err(TaskError::Other(format!(
            "cannot parse filter expression {input:?}: unexpected end of expression"
        ))),
    }
}

// ─── Errors ─────────────────────────────────────────────────────────────────

/// A parse failure, carrying enough to name the offending input.
///
/// `detail` is set by the parsers that know what went wrong (an unknown field,
/// a malformed tag); the rest leave it empty and the position speaks for itself.
#[derive(Debug, Clone, PartialEq)]
struct ExprError {
    at: String,
    detail: Option<String>,
}

impl ExprError {
    fn bare(at: &str) -> Self {
        Self {
            at: at.to_owned(),
            detail: None,
        }
    }

    fn detailed(at: &str, detail: impl Into<String>) -> Self {
        Self {
            at: at.to_owned(),
            detail: Some(detail.into()),
        }
    }

    fn render(&self, full: &str) -> String {
        let what = match &self.detail {
            Some(detail) => detail.clone(),
            None if self.at.is_empty() => "unexpected end of expression".to_owned(),
            None => format!("unexpected input at {:?}", self.at),
        };
        format!("cannot parse filter expression {full:?}: {what}")
    }
}

impl ParseError<&str> for ExprError {
    fn from_error_kind(input: &str, _kind: ErrorKind) -> Self {
        Self::bare(input)
    }

    fn append(_input: &str, _kind: ErrorKind, other: Self) -> Self {
        other
    }

    /// `alt` folds its branches' errors through this. Prefer whichever branch
    /// managed to say something specific over a bare "unexpected input".
    fn or(self, other: Self) -> Self {
        if other.detail.is_some() || self.detail.is_none() {
            other
        } else {
            self
        }
    }
}

type PResult<'a, T> = IResult<&'a str, T, ExprError>;

/// An unrecoverable error: the input committed to a form and got it wrong.
fn fail(at: &str, detail: impl Into<String>) -> nom::Err<ExprError> {
    nom::Err::Failure(ExprError::detailed(at, detail))
}

/// Turns a missing-but-required piece into a [`fail`], while letting a nested
/// failure (which already has a better message) through untouched.
fn require<'a, T>(result: PResult<'a, T>, at: &str, detail: impl Into<String>) -> PResult<'a, T> {
    match result {
        Ok(ok) => Ok(ok),
        Err(e @ nom::Err::Failure(_)) => Err(e),
        Err(_) => Err(fail(at, detail)),
    }
}

// ─── Grammar ────────────────────────────────────────────────────────────────
//
// expr    := or
// or      := and (('or' | '|') and)*
// and     := unary (('and' | '&')? unary)*
// unary   := ('not' | '!') unary | '(' expr ')' | atom
//
// Adjacency is AND, so precedence runs not > and > or and `a or b c` is
// `a or (b and c)`.
//
// The grammar is recursive descent, so nesting depth is stack depth: every `(`
// and every `not` costs a frame. `depth` is threaded through the recursive
// productions and checked in `unary`, which every nesting level passes through
// exactly once.

/// How deeply an expression may nest before it is rejected.
///
/// A stack overflow is a `SIGABRT`, not a catchable panic — the whole process
/// dies, which for the MCP server means every session, not just the request
/// that sent the filter.
///
/// The number is set from measurement, not taste: these combinators cost
/// 8–16 KB of stack per level in a debug build (63 levels needed between
/// 512 KB and 1 MB), so 32 levels stays well inside the 2 MB a tokio worker
/// or test thread gets, let alone the 8 MB of a main thread. It is also far
/// beyond any real query — hand-written filters rarely nest past 3, and a
/// generated one a handful.
const MAX_DEPTH: usize = 32;

/// Skips leading whitespace before `p`, pinning the error type along the way.
fn spaced<'a, O, P>(p: P) -> impl Parser<&'a str, Output = O, Error = ExprError>
where
    P: Parser<&'a str, Output = O, Error = ExprError>,
{
    preceded(multispace0, p)
}

fn expr(input: &str, depth: usize) -> PResult<'_, Expr> {
    or_expr(input, depth)
}

fn or_expr(input: &str, depth: usize) -> PResult<'_, Expr> {
    let (rest, first) = and_expr(input, depth)?;
    let (rest, more) = many0(preceded(or_op, cut(move |i| and_expr(i, depth)))).parse(rest)?;
    Ok((rest, combine(first, more, Expr::Or)))
}

fn and_expr(input: &str, depth: usize) -> PResult<'_, Expr> {
    let (rest, first) = unary(input, depth)?;
    // An explicit `and` commits (a missing right-hand side is an error); plain
    // adjacency does not, so the loop can end at `or`, `)` or end of input.
    let (rest, more) = many0(alt((
        preceded(and_op, cut(move |i| unary(i, depth))),
        move |i| unary(i, depth),
    )))
    .parse(rest)?;
    Ok((rest, combine(first, more, Expr::And)))
}

fn unary(input: &str, depth: usize) -> PResult<'_, Expr> {
    if depth >= MAX_DEPTH {
        return Err(fail(
            input,
            format!("expression nests more than {MAX_DEPTH} levels deep"),
        ));
    }
    let (rest, negated) = opt(not_op).parse(input)?;
    if negated.is_some() {
        let (rest, inner) = cut(move |i| unary(i, depth + 1)).parse(rest)?;
        return Ok((rest, Expr::Not(Box::new(inner))));
    }
    alt((move |i| group(i, depth), atom_expr)).parse(input)
}

fn group(input: &str, depth: usize) -> PResult<'_, Expr> {
    let (rest, _) = spaced(nom_char('(')).parse(input)?;
    let (rest, inner) = cut(move |i| expr(i, depth + 1)).parse(rest)?;
    let (rest, close) = opt(spaced(nom_char(')'))).parse(rest)?;
    if close.is_none() {
        return Err(fail(rest, "missing closing ')'"));
    }
    Ok((rest, inner))
}

fn combine(first: Expr, more: Vec<Expr>, make: fn(Vec<Expr>) -> Expr) -> Expr {
    if more.is_empty() {
        return first;
    }
    let mut parts = Vec::with_capacity(more.len() + 1);
    parts.push(first);
    parts.extend(more);
    make(parts)
}

/// Matches an operator word only when it stands alone, so that `order` is a
/// search term rather than `or` followed by `der`.
fn keyword<'a>(word: &'static str) -> impl FnMut(&'a str) -> PResult<'a, &'a str> {
    move |input: &'a str| {
        let (rest, matched) = tag_no_case(word).parse(input)?;
        match rest.chars().next() {
            None => Ok((rest, matched)),
            Some(c) if c.is_whitespace() || c == '(' || c == ')' => Ok((rest, matched)),
            Some(_) => Err(nom::Err::Error(ExprError::bare(input))),
        }
    }
}

fn or_op(input: &str) -> PResult<'_, ()> {
    spaced(alt((keyword("or"), recognize(nom_char('|')))))
        .parse(input)
        .map(|(rest, _)| (rest, ()))
}

fn and_op(input: &str) -> PResult<'_, ()> {
    spaced(alt((keyword("and"), recognize(nom_char('&')))))
        .parse(input)
        .map(|(rest, _)| (rest, ()))
}

fn not_op(input: &str) -> PResult<'_, ()> {
    spaced(alt((keyword("not"), recognize(nom_char('!')))))
        .parse(input)
        .map(|(rest, _)| (rest, ()))
}

// ─── Atoms ──────────────────────────────────────────────────────────────────

fn atom_expr(input: &str) -> PResult<'_, Expr> {
    spaced(alt((tag_atom, is_atom, has_atom, field_atom, search_atom))).parse(input)
}

/// `+tag` / `-tag`. The sign is the only thing that distinguishes a tag from a
/// search term, so once it is seen the rest of the token must be a valid tag.
fn tag_atom(input: &str) -> PResult<'_, Expr> {
    let (rest, sign) = alt((nom_char('+'), nom_char('-'))).parse(input)?;
    let (rest, name) = opt(take_while1_value).parse(rest)?;
    let Some(name) = name else {
        return Err(fail(
            input,
            format!("{sign:?} must be followed by a tag name"),
        ));
    };
    // `-due<7d` is someone reaching for negation, not writing a tag. Saying
    // "segments may only contain letters" sends them to fix a tag they never
    // meant to write, so name the mistake instead.
    if let Some(op) = name.find([':', '<', '>']) {
        let suggestion = if sign == '-' {
            format!("not {name}")
        } else {
            name.to_owned()
        };
        return Err(fail(
            input,
            format!(
                "{sign:?} takes a tag name, but {:?} is a {} predicate — write {suggestion:?}",
                name,
                &name[op..op + 1],
            ),
        ));
    }
    tag::validate_tag(name).map_err(|e| fail(input, e))?;
    let atom = Expr::Atom(Atom::Tag(name.to_owned()));
    Ok((
        rest,
        if sign == '-' {
            Expr::Not(Box::new(atom))
        } else {
            atom
        },
    ))
}

fn is_atom(input: &str) -> PResult<'_, Expr> {
    let (rest, _) = nom_tag("is:").parse(input)?;
    let (rest, name) = opt(take_while1_value).parse(rest)?;
    let predicate = name
        .and_then(|n| Named::ALL.iter().find(|(known, _)| *known == n))
        .map(|(_, p)| *p);
    match predicate {
        Some(p) => Ok((rest, Expr::Atom(Atom::Is(p)))),
        None => {
            let known: Vec<&str> = Named::ALL.iter().map(|(n, _)| *n).collect();
            Err(fail(
                input,
                format!(
                    "unknown is: predicate {:?} — expected one of {}",
                    name.unwrap_or(""),
                    known.join(", ")
                ),
            ))
        }
    }
}

/// `has:field` and its negation `no:field`.
fn has_atom(input: &str) -> PResult<'_, Expr> {
    let (rest, keyword) = alt((nom_tag("has:"), nom_tag("no:"))).parse(input)?;
    let (rest, name) = opt(field_name).parse(rest)?;
    let field = name.and_then(resolve_field).ok_or_else(|| {
        fail(
            input,
            format!(
                "{keyword} expects a field name, got {:?}",
                name.unwrap_or("")
            ),
        )
    })?;
    let atom = Expr::Atom(Atom::Has(field));
    Ok((
        rest,
        if keyword == "no:" {
            Expr::Not(Box::new(atom))
        } else {
            atom
        },
    ))
}

/// `field:value`, `field:a,b`, `field:low..high`, `field<v`, `field<=v`, …
///
/// A token only reaches the search branch if it has no operator at all, so an
/// unrecognised field name in an obviously-a-predicate token is an error rather
/// than a silent full-text search for `foo:bar`.
fn field_atom(input: &str) -> PResult<'_, Expr> {
    let (rest, name) = field_name(input)?;
    let (rest, op) = field_op(rest)?;
    let Some(field) = resolve_field(name) else {
        return Err(fail(input, format!("unknown field {name:?}")));
    };
    match op {
        None => colon_predicate(input, rest, field),
        Some(cmp) => {
            if !field.is_ordered() {
                return Err(fail(
                    input,
                    format!(
                        "field {:?} does not support the {} operator",
                        field.name(),
                        cmp.symbol()
                    ),
                ));
            }
            let (rest, value) = require(
                value_token(rest),
                input,
                format!("missing value after {}{}", field.name(), cmp.symbol()),
            )?;
            Ok((
                rest,
                Expr::Atom(Atom::Compare {
                    field,
                    op: cmp,
                    value,
                }),
            ))
        }
    }
}

/// Everything after `field:`.
fn colon_predicate<'a>(whole: &'a str, input: &'a str, field: Field) -> PResult<'a, Expr> {
    let missing = format!("missing value after {}:", field.name());

    // A text field's `:` scopes a search rather than testing equality.
    if let Some(text) = field.as_text() {
        let (rest, terms) = require(
            separated_list1(nom_char(','), scoped_term).parse(input),
            whole,
            missing,
        )?;
        let mut parts = Vec::with_capacity(terms.len());
        for term in terms {
            if term.text.is_empty() {
                return Err(fail(
                    whole,
                    format!("empty search term after {}:", field.name()),
                ));
            }
            parts.push(Expr::Atom(Atom::Search {
                field: Some(text),
                term: term.text,
                prefix: term.prefix,
            }));
        }
        return Ok((rest, combine_list(parts, Expr::Or)));
    }

    let (rest, first) = require(value_token(input), whole, missing.clone())?;

    let (after_dots, dots) = opt(nom_tag("..")).parse(rest)?;
    if dots.is_some() {
        if !field.is_ordered() {
            return Err(fail(
                whole,
                format!("field {:?} does not support ranges", field.name()),
            ));
        }
        let (rest, high) = require(
            value_token(after_dots),
            whole,
            format!("missing upper bound after {}:{}..", field.name(), first.raw),
        )?;
        return Ok((
            rest,
            Expr::Atom(Atom::Range {
                field,
                low: first,
                high,
            }),
        ));
    }

    let (rest, more) = many0(preceded(nom_char(','), |i| {
        require(value_token(i), whole, missing.clone())
    }))
    .parse(rest)?;
    let mut values = Vec::with_capacity(more.len() + 1);
    values.push(first);
    values.extend(more);

    // `tag:` is the long form of `+tag`, down to the hierarchy semantics, so it
    // produces the same atom instead of a second way to say the same thing.
    if field == Field::Tag {
        let mut parts = Vec::with_capacity(values.len());
        for value in values {
            tag::validate_tag(&value.raw).map_err(|e| fail(whole, e))?;
            parts.push(Expr::Atom(Atom::Tag(value.raw)));
        }
        return Ok((rest, combine_list(parts, Expr::Or)));
    }

    Ok((rest, Expr::Atom(Atom::Equals { field, values })))
}

fn combine_list(mut parts: Vec<Expr>, make: fn(Vec<Expr>) -> Expr) -> Expr {
    if parts.len() == 1 {
        parts.pop().expect("length checked")
    } else {
        make(parts)
    }
}

fn field_name(input: &str) -> PResult<'_, &str> {
    recognize((
        satisfy(|c: char| c.is_ascii_alphabetic()),
        take_while(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'),
    ))
    .parse(input)
}

/// The operator that follows a field name: `None` for `:`, `Some(op)` for an
/// ordered comparison. Fails (recoverably) when the token is not a predicate.
fn field_op(input: &str) -> PResult<'_, Option<CompareOp>> {
    alt((
        nom_tag("<=").map(|_| Some(CompareOp::Le)),
        nom_tag(">=").map(|_| Some(CompareOp::Ge)),
        nom_tag("<").map(|_| Some(CompareOp::Lt)),
        nom_tag(">").map(|_| Some(CompareOp::Gt)),
        nom_tag(":").map(|_| None),
    ))
    .parse(input)
}

fn resolve_field(name: &str) -> Option<Field> {
    Some(match name {
        "status" => Field::Status,
        "priority" => Field::Priority,
        "due" => Field::Due,
        "start" => Field::Start,
        "completed" => Field::Completed,
        "created" => Field::Created,
        "updated" => Field::Updated,
        "assignee" => Field::Assignee,
        "slug" => Field::Slug,
        "parent" => Field::Parent,
        "tag" => Field::Tag,
        "context" => Field::Context,
        "user" => Field::User,
        "title" => Field::Title,
        "description" => Field::Description,
        "notes" => Field::Notes,
        "url" => Field::Url,
        "score" => Field::Score,
        other => {
            let key = other.strip_prefix("data.")?;
            if key.is_empty() {
                return None;
            }
            Field::Data(key.to_owned())
        }
    })
}

// ─── Terminals ──────────────────────────────────────────────────────────────

/// A search term with the flags the grammar attaches to it.
struct Term {
    text: String,
    prefix: bool,
    quoted: bool,
}

/// The default atom: a bare word, a quoted phrase, or either with a `*` suffix.
fn search_atom(input: &str) -> PResult<'_, Expr> {
    let (rest, term) = free_term(input)?;
    if !term.quoted && is_reserved(&term.text) {
        // Recoverable: `and_expr` relies on this to stop at `or`, and reporting
        // it as a failure would break `a or b`.
        return Err(nom::Err::Error(ExprError::detailed(
            input,
            format!(
                "unexpected operator {:?} — quote it to search for the word",
                term.text
            ),
        )));
    }
    if term.text.is_empty() {
        return Err(fail(input, "empty search term"));
    }
    Ok((
        rest,
        Expr::Atom(Atom::Search {
            field: None,
            term: term.text,
            prefix: term.prefix,
        }),
    ))
}

fn is_reserved(word: &str) -> bool {
    ["and", "or", "not"]
        .iter()
        .any(|kw| word.eq_ignore_ascii_case(kw))
}

/// A term in the bare slot, where a comma is just another character.
fn free_term(input: &str) -> PResult<'_, Term> {
    term(input, true)
}

/// A term after `textfield:`, where a comma separates the members of a set.
fn scoped_term(input: &str) -> PResult<'_, Term> {
    term(input, false)
}

fn term(input: &str, allow_comma: bool) -> PResult<'_, Term> {
    let (rest, phrase) = opt(quoted_string).parse(input)?;
    if let Some(text) = phrase {
        let (rest, star) = opt(nom_char('*')).parse(rest)?;
        return Ok((
            rest,
            Term {
                text,
                prefix: star.is_some(),
                quoted: true,
            },
        ));
    }
    let (rest, word) = take_while1_term(input, allow_comma)?;
    // A trailing `*` is the prefix marker; anywhere else it is part of the term.
    match word.strip_suffix('*') {
        Some(stem) if !stem.is_empty() => Ok((
            rest,
            Term {
                text: stem.to_owned(),
                prefix: true,
                quoted: false,
            },
        )),
        _ => Ok((
            rest,
            Term {
                text: word.to_owned(),
                prefix: false,
                quoted: false,
            },
        )),
    }
}

fn value_token(input: &str) -> PResult<'_, Value> {
    let (rest, phrase) = opt(quoted_string).parse(input)?;
    if let Some(raw) = phrase {
        return Ok((rest, Value { raw, quoted: true }));
    }
    let (rest, raw) = bare_value(input)?;
    Ok((
        rest,
        Value {
            raw: raw.to_owned(),
            quoted: false,
        },
    ))
}

/// Characters that can appear unquoted inside a term. Whitespace separates
/// atoms; parentheses and quotes are structural; and the symbol operators are
/// excluded so that `a&b` and `a|b` compose without needing spaces. A term that
/// really contains one of them has to be quoted.
fn is_term_char(c: char, allow_comma: bool) -> bool {
    !c.is_whitespace()
        && !matches!(c, '(' | ')' | '"' | '&' | '|' | '!')
        && (allow_comma || c != ',')
}

fn take_while1_term(input: &str, allow_comma: bool) -> PResult<'_, &str> {
    let end = input
        .char_indices()
        .find(|(_, c)| !is_term_char(*c, allow_comma))
        .map(|(i, _)| i)
        .unwrap_or(input.len());
    if end == 0 {
        return Err(nom::Err::Error(ExprError::bare(input)));
    }
    Ok((&input[end..], &input[..end]))
}

/// A tag name or a plain value: a term that also stops at a comma.
fn take_while1_value(input: &str) -> PResult<'_, &str> {
    take_while1_term(input, false)
}

/// An unquoted value, which additionally stops at the `..` range separator so
/// that `2026-07-01..2026-07-31` splits into two dates.
fn bare_value(input: &str) -> PResult<'_, &str> {
    let mut end = input.len();
    for (i, c) in input.char_indices() {
        if !is_term_char(c, false) || input[i..].starts_with("..") {
            end = i;
            break;
        }
    }
    if end == 0 {
        return Err(nom::Err::Error(ExprError::bare(input)));
    }
    Ok((&input[end..], &input[..end]))
}

/// A double-quoted string. A backslash escapes the character that follows it,
/// which is the only way to write a `"` inside a phrase.
fn quoted_string(input: &str) -> PResult<'_, String> {
    let (rest, _) = nom_char('"').parse(input)?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some((_, escaped)) => out.push(escaped),
                None => return Err(fail(input, "unterminated quoted string")),
            },
            '"' => return Ok((&rest[i + c.len_utf8()..], out)),
            _ => out.push(c),
        }
    }
    Err(fail(input, "unterminated quoted string"))
}

// ─── Rendering ──────────────────────────────────────────────────────────────
//
// `Display` emits the canonical spelling of an expression, which re-parses to
// exactly the same tree. Grouping is parenthesised wherever the AST nests, even
// when precedence would not require it, so printing never flattens the tree.

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::And(parts) => write_joined(f, parts, " and ", |p| {
                matches!(p, Expr::And(_) | Expr::Or(_))
            }),
            Expr::Or(parts) => write_joined(f, parts, " or ", |p| matches!(p, Expr::Or(_))),
            Expr::Not(inner) => {
                write!(f, "not ")?;
                write_operand(f, inner, !matches!(**inner, Expr::Atom(_)))
            }
            Expr::Atom(atom) => write!(f, "{atom}"),
        }
    }
}

fn write_joined(
    f: &mut fmt::Formatter<'_>,
    parts: &[Expr],
    separator: &str,
    parenthesise: impl Fn(&Expr) -> bool,
) -> fmt::Result {
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            write!(f, "{separator}")?;
        }
        write_operand(f, part, parenthesise(part))?;
    }
    Ok(())
}

fn write_operand(f: &mut fmt::Formatter<'_>, part: &Expr, parenthesise: bool) -> fmt::Result {
    if parenthesise {
        write!(f, "({part})")
    } else {
        write!(f, "{part}")
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Atom::Search {
                field,
                term,
                prefix,
            } => {
                if let Some(field) = field {
                    write!(f, "{}:", field.name())?;
                }
                write!(f, "{}", render_term(term))?;
                if *prefix {
                    write!(f, "*")?;
                }
                Ok(())
            }
            Atom::Tag(name) => write!(f, "+{name}"),
            Atom::Equals { field, values } => {
                write!(f, "{}:", field.name())?;
                for (i, value) in values.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{value}")?;
                }
                Ok(())
            }
            Atom::Compare { field, op, value } => {
                write!(f, "{}{}{}", field.name(), op.symbol(), value)
            }
            Atom::Range { field, low, high } => write!(f, "{}:{}..{}", field.name(), low, high),
            Atom::Has(field) => write!(f, "has:{}", field.name()),
            Atom::Is(named) => write!(f, "is:{}", named.name()),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.quoted {
            write!(f, "{}", quote(&self.raw))
        } else {
            write!(f, "{}", self.raw)
        }
    }
}

/// Renders a search term, quoting it whenever the bare spelling would parse as
/// something else (an operator word, a sigil, another kind of atom).
fn render_term(term: &str) -> String {
    let needs_quotes = term.is_empty()
        || is_reserved(term)
        || term.chars().any(|c| {
            c.is_whitespace() || matches!(c, '(' | ')' | '"' | '&' | '|' | '!' | ':' | ',' | '*')
        })
        || term.starts_with(['+', '-', '<', '>']);
    if needs_quotes {
        quote(term)
    } else {
        term.to_owned()
    }
}

fn quote(raw: &str) -> String {
    let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> Expr {
        parse(input).unwrap_or_else(|e| panic!("{input:?} should parse: {e}"))
    }

    fn err(input: &str) -> String {
        parse(input)
            .expect_err(&format!("{input:?} should not parse"))
            .to_string()
    }

    fn search(term: &str) -> Expr {
        Expr::Atom(Atom::Search {
            field: None,
            term: term.to_owned(),
            prefix: false,
        })
    }

    fn tagged(name: &str) -> Expr {
        Expr::Atom(Atom::Tag(name.to_owned()))
    }

    // ── Search atoms ────────────────────────────────────────────────────────

    #[test]
    fn bare_word_is_a_search() {
        assert_eq!(ok("archive"), search("archive"));
    }

    #[test]
    fn quoted_phrase_stays_one_term() {
        assert_eq!(ok(r#""cold tier""#), search("cold tier"));
    }

    #[test]
    fn escaped_quote_inside_a_phrase() {
        assert_eq!(ok(r#""say \"hi\"""#), search(r#"say "hi""#));
    }

    #[test]
    fn trailing_star_is_a_prefix_search() {
        assert_eq!(
            ok("arch*"),
            Expr::Atom(Atom::Search {
                field: None,
                term: "arch".to_owned(),
                prefix: true,
            })
        );
    }

    #[test]
    fn phrase_can_be_a_prefix_search() {
        assert_eq!(
            ok(r#""cold tie"*"#),
            Expr::Atom(Atom::Search {
                field: None,
                term: "cold tie".to_owned(),
                prefix: true,
            })
        );
    }

    #[test]
    fn search_scoped_to_a_text_field() {
        for (input, field) in [
            ("title:rename", TextField::Title),
            ("description:rename", TextField::Description),
            ("notes:rename", TextField::Notes),
            ("url:rename", TextField::Url),
        ] {
            assert_eq!(
                ok(input),
                Expr::Atom(Atom::Search {
                    field: Some(field),
                    term: "rename".to_owned(),
                    prefix: false,
                }),
                "{input}"
            );
        }
    }

    #[test]
    fn slug_is_an_identifier_not_a_search() {
        // A slug names one task, so `slug:` matches it exactly rather than
        // searching within it — `slug:water` must not match "water-plants".
        assert_eq!(
            ok("slug:water-plants"),
            Expr::Atom(Atom::Equals {
                field: Field::Slug,
                values: vec![Value::word("water-plants")],
            })
        );
        assert_eq!(
            ok("slug:a,b"),
            Expr::Atom(Atom::Equals {
                field: Field::Slug,
                values: vec![Value::word("a"), Value::word("b")],
            })
        );
        assert!(err("slug<x").contains("does not support the < operator"));
    }

    #[test]
    fn scoped_search_takes_prefixes_phrases_and_sets() {
        assert_eq!(
            ok("notes:fts*"),
            Expr::Atom(Atom::Search {
                field: Some(TextField::Notes),
                term: "fts".to_owned(),
                prefix: true,
            })
        );
        assert_eq!(
            ok(r#"title:"cold tier""#),
            Expr::Atom(Atom::Search {
                field: Some(TextField::Title),
                term: "cold tier".to_owned(),
                prefix: false,
            })
        );
        assert_eq!(
            ok("title:a,b"),
            Expr::Or(vec![
                Expr::Atom(Atom::Search {
                    field: Some(TextField::Title),
                    term: "a".to_owned(),
                    prefix: false,
                }),
                Expr::Atom(Atom::Search {
                    field: Some(TextField::Title),
                    term: "b".to_owned(),
                    prefix: false,
                }),
            ])
        );
    }

    // ── Tag atoms ───────────────────────────────────────────────────────────

    #[test]
    fn required_tag() {
        assert_eq!(ok("+bug"), tagged("bug"));
        assert_eq!(ok("+@ai/next"), tagged("@ai/next"));
        assert_eq!(ok("+#office/printer"), tagged("#office/printer"));
    }

    #[test]
    fn excluded_tag_is_a_negated_tag() {
        assert_eq!(ok("-#printer"), Expr::Not(Box::new(tagged("#printer"))));
    }

    #[test]
    fn tag_field_is_the_long_form() {
        assert_eq!(ok("tag:@work"), tagged("@work"));
        assert_eq!(
            ok("tag:@work,@home"),
            Expr::Or(vec![tagged("@work"), tagged("@home")])
        );
    }

    #[test]
    fn malformed_tag_is_a_parse_error() {
        let message = err("+1bug");
        assert!(message.contains("+1bug"), "{message}");
        assert!(message.contains("must start with a letter"), "{message}");

        assert!(err("-work//backend").contains("empty path segment"));
        assert!(err("tag:__reserved").contains("__"));
        assert!(err("+").contains("must be followed by a tag name"));
    }

    #[test]
    fn sign_on_a_predicate_suggests_the_right_spelling() {
        // Reaching for negation with `-` is the likely mistake here; a
        // complaint about tag syntax would send the user to fix the wrong thing.
        let message = err("-due<7d");
        assert!(message.contains("is a < predicate"), "{message}");
        assert!(message.contains(r#""not due<7d""#), "{message}");

        let message = err("+status:open");
        assert!(message.contains("is a : predicate"), "{message}");
        assert!(message.contains(r#""status:open""#), "{message}");

        // A genuinely malformed tag still gets the tag-validation message.
        assert!(err("+1bug").contains("must start with a letter"));
    }

    // ── Field predicates ────────────────────────────────────────────────────

    #[test]
    fn equality_and_sets() {
        assert_eq!(
            ok("priority:high"),
            Expr::Atom(Atom::Equals {
                field: Field::Priority,
                values: vec![Value::word("high")],
            })
        );
        assert_eq!(
            ok("status:open,started"),
            Expr::Atom(Atom::Equals {
                field: Field::Status,
                values: vec![Value::word("open"), Value::word("started")],
            })
        );
    }

    #[test]
    fn ordered_comparisons() {
        for (input, op) in [
            ("due<+7d", CompareOp::Lt),
            ("due<=+7d", CompareOp::Le),
            ("due>+7d", CompareOp::Gt),
            ("due>=+7d", CompareOp::Ge),
        ] {
            assert_eq!(
                ok(input),
                Expr::Atom(Atom::Compare {
                    field: Field::Due,
                    op,
                    value: Value::word("+7d"),
                }),
                "{input}"
            );
        }
    }

    #[test]
    fn dates_and_durations_are_kept_raw() {
        for raw in [
            "2026-08-10",
            "+7d",
            "-2w",
            "+3m",
            "today",
            "tomorrow",
            "eow",
            "eom",
        ] {
            assert_eq!(
                ok(&format!("due:{raw}")),
                Expr::Atom(Atom::Equals {
                    field: Field::Due,
                    values: vec![Value::word(raw)],
                }),
                "{raw}"
            );
        }
        assert_eq!(
            ok(r#"due<"next monday""#),
            Expr::Atom(Atom::Compare {
                field: Field::Due,
                op: CompareOp::Lt,
                value: Value::quoted("next monday"),
            })
        );
    }

    #[test]
    fn inclusive_range() {
        assert_eq!(
            ok("completed:2026-07-01..2026-07-31"),
            Expr::Atom(Atom::Range {
                field: Field::Completed,
                low: Value::word("2026-07-01"),
                high: Value::word("2026-07-31"),
            })
        );
    }

    #[test]
    fn has_and_no() {
        assert_eq!(ok("has:assignee"), Expr::Atom(Atom::Has(Field::Assignee)));
        assert_eq!(
            ok("no:assignee"),
            Expr::Not(Box::new(Expr::Atom(Atom::Has(Field::Assignee))))
        );
        assert_eq!(
            ok("has:data.forgejo_issue"),
            Expr::Atom(Atom::Has(Field::Data("forgejo_issue".to_owned())))
        );
    }

    #[test]
    fn named_predicates() {
        for (name, predicate) in Named::ALL {
            assert_eq!(
                ok(&format!("is:{name}")),
                Expr::Atom(Atom::Is(predicate)),
                "{name}"
            );
        }
    }

    #[test]
    fn state_override_fields_keep_their_meaning() {
        assert_eq!(
            ok("parent:filter-language-research"),
            Expr::Atom(Atom::Equals {
                field: Field::Parent,
                values: vec![Value::word("filter-language-research")],
            })
        );
        assert_eq!(
            ok("context:@work"),
            Expr::Atom(Atom::Equals {
                field: Field::Context,
                values: vec![Value::word("@work")],
            })
        );
        assert_eq!(
            ok("user:alice,bob"),
            Expr::Atom(Atom::Equals {
                field: Field::User,
                values: vec![Value::word("alice"), Value::word("bob")],
            })
        );
    }

    #[test]
    fn data_and_score_fields() {
        assert_eq!(
            ok("data.sprint:12"),
            Expr::Atom(Atom::Equals {
                field: Field::Data("sprint".to_owned()),
                values: vec![Value::word("12")],
            })
        );
        assert_eq!(
            ok("score>=3.5"),
            Expr::Atom(Atom::Compare {
                field: Field::Score,
                op: CompareOp::Ge,
                value: Value::word("3.5"),
            })
        );
    }

    #[test]
    fn unsupported_operators_are_rejected() {
        assert!(err("status<open").contains("does not support the < operator"));
        assert!(err("title>x").contains("does not support the > operator"));
        assert!(err("status:open..done").contains("does not support ranges"));
    }

    #[test]
    fn unknown_field_is_an_error_not_a_search() {
        let message = err("colour:red");
        assert!(message.contains("colour:red"), "{message}");
        assert!(message.contains("unknown field \"colour\""), "{message}");
        assert!(err("is:sideways").contains("unknown is: predicate"));
        assert!(err("has:colour").contains("expects a field name"));
    }

    #[test]
    fn missing_values_are_reported() {
        assert!(err("status:").contains("missing value after status:"));
        assert!(err("due<=").contains("missing value after due<="));
        assert!(err("completed:2026-07-01..").contains("missing upper bound"));
        assert!(err("title:").contains("missing value after title:"));
    }

    // ── Booleans ────────────────────────────────────────────────────────────

    #[test]
    fn adjacency_is_and() {
        assert_eq!(
            ok("segment prune"),
            Expr::And(vec![search("segment"), search("prune")])
        );
    }

    #[test]
    fn both_operator_spellings() {
        let expected_and = Expr::And(vec![search("a"), search("b")]);
        assert_eq!(ok("a and b"), expected_and);
        assert_eq!(ok("a & b"), expected_and);
        assert_eq!(ok("a&b"), expected_and);

        let expected_or = Expr::Or(vec![search("a"), search("b")]);
        assert_eq!(ok("a or b"), expected_or);
        assert_eq!(ok("a | b"), expected_or);

        let expected_not = Expr::Not(Box::new(search("a")));
        assert_eq!(ok("not a"), expected_not);
        assert_eq!(ok("!a"), expected_not);
    }

    #[test]
    fn operator_words_are_case_insensitive() {
        assert_eq!(ok("a OR b"), Expr::Or(vec![search("a"), search("b")]));
        assert_eq!(ok("a And b"), Expr::And(vec![search("a"), search("b")]));
        assert_eq!(ok("NOT a"), Expr::Not(Box::new(search("a"))));
    }

    #[test]
    fn operator_words_need_a_boundary() {
        assert_eq!(ok("order"), search("order"));
        assert_eq!(ok("android"), search("android"));
        assert_eq!(ok("nothing"), search("nothing"));
    }

    #[test]
    fn not_binds_tighter_than_and() {
        assert_eq!(
            ok("not a b"),
            Expr::And(vec![Expr::Not(Box::new(search("a"))), search("b")])
        );
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let expected = Expr::Or(vec![search("a"), Expr::And(vec![search("b"), search("c")])]);
        assert_eq!(ok("a or b c"), expected);
        assert_eq!(ok("a or (b and c)"), expected);
    }

    #[test]
    fn parentheses_group() {
        assert_eq!(
            ok("(a or b) and c"),
            Expr::And(vec![Expr::Or(vec![search("a"), search("b")]), search("c"),])
        );
        assert_eq!(
            ok("(+@work or +@home) and not +#printer"),
            Expr::And(vec![
                Expr::Or(vec![tagged("@work"), tagged("@home")]),
                Expr::Not(Box::new(tagged("#printer"))),
            ])
        );
        assert_eq!(
            ok("(+@work | +@home) & !+#printer"),
            ok("(+@work or +@home) and not +#printer")
        );
    }

    #[test]
    fn negated_group() {
        assert_eq!(
            ok("!(a b)"),
            Expr::Not(Box::new(Expr::And(vec![search("a"), search("b")])))
        );
    }

    #[test]
    fn reserved_words_can_be_searched_when_quoted() {
        assert_eq!(
            ok(r#""not" +bug"#),
            Expr::And(vec![search("not"), tagged("bug")])
        );
        assert_eq!(ok(r#""and""#), search("and"));
        assert_eq!(ok(r#""or""#), search("or"));
    }

    #[test]
    fn bare_reserved_word_is_an_operator() {
        let message = err("or");
        assert!(message.contains("unexpected operator \"or\""), "{message}");
        assert!(message.contains("quote it to search"), "{message}");
    }

    // ── Malformed input ─────────────────────────────────────────────────────

    #[test]
    fn empty_input_matches_everything() {
        assert_eq!(parse("").unwrap(), Expr::And(vec![]));
        assert_eq!(parse("   \t ").unwrap(), Expr::And(vec![]));
    }

    #[test]
    fn dangling_operators() {
        assert!(err("a or").contains("unexpected end of expression"));
        assert!(err("a and").contains("unexpected end of expression"));
        assert!(err("not").contains("unexpected end of expression"));
    }

    #[test]
    fn unbalanced_parentheses() {
        assert!(err("(a or b").contains("missing closing ')'"));
        let message = err("a)");
        assert!(message.contains("unexpected input at \")\""), "{message}");
    }

    #[test]
    fn deep_nesting_is_rejected_not_fatal() {
        // Before the depth bound this aborted the process with a stack
        // overflow, which no caller can catch. These inputs are far past the
        // limit but far below the ~300 levels that used to be fatal, so the
        // assertion is that an ERROR comes back — a regression here kills the
        // whole test binary rather than failing this case.
        for depth in [MAX_DEPTH + 1, 200] {
            let parens = format!("{}a{}", "(".repeat(depth), ")".repeat(depth));
            assert!(
                err(&parens).contains("nests more than"),
                "{depth} nested parens should be refused"
            );

            let nots = format!("{}a", "not ".repeat(depth));
            assert!(
                err(&nots).contains("nests more than"),
                "{depth} nested nots should be refused"
            );

            // Unbalanced input takes a different path through the parser and
            // must be bounded too.
            assert!(err(&"(".repeat(depth)).contains("nests more than"));
        }
    }

    #[test]
    fn realistic_nesting_still_parses() {
        // The bound must not get in the way of an expression a person or an
        // agent would actually write.
        let expr = ok("((+@work or +@home) and not (due<+7d or (priority:high and is:blocked)))");
        assert_eq!(ok(&expr.to_string()), expr, "round-trip survives nesting");

        // Right at the limit: MAX_DEPTH levels are allowed, one more is not.
        let at_limit = format!(
            "{}a{}",
            "(".repeat(MAX_DEPTH - 1),
            ")".repeat(MAX_DEPTH - 1)
        );
        assert_eq!(ok(&at_limit), search("a"));
    }

    #[test]
    fn unterminated_quote() {
        let message = err(r#""cold tier"#);
        assert!(message.contains("unterminated quoted string"), "{message}");
    }

    #[test]
    fn error_names_the_whole_input() {
        let message = err("+1bug");
        assert!(
            message.starts_with("cannot parse filter expression \"+1bug\""),
            "{message}"
        );
    }

    // ── Rendering and round-trips ───────────────────────────────────────────

    #[test]
    fn nested_expression_round_trips() {
        let input = r#"(+@work or +@home) and not +#printer and (due<=+3d or priority:high) and "cold tier" and title:arch* and is:overdue and no:assignee and completed:2026-07-01..2026-07-31 and status:open,started"#;
        let parsed = ok(input);
        let printed = parsed.to_string();
        assert_eq!(
            ok(&printed),
            parsed,
            "printed form {printed:?} did not re-parse to the same tree"
        );
    }

    #[test]
    fn printing_preserves_nesting_that_precedence_would_flatten() {
        for input in [
            "a or (b or c)",
            "(a b) c",
            "not (a or b)",
            "a and (b or c)",
            "(a or b) and (c or d)",
        ] {
            let parsed = ok(input);
            assert_eq!(ok(&parsed.to_string()), parsed, "{input}");
        }
    }

    #[test]
    fn printing_quotes_terms_that_would_otherwise_reparse_differently() {
        for term in [
            "not",
            "and",
            "or",
            "two words",
            "a:b",
            "star*",
            "+plus",
            "a&b",
            "wow!",
        ] {
            let expr = search(term);
            assert_eq!(ok(&expr.to_string()), expr, "{term}");
        }
    }

    #[test]
    fn canonical_spelling() {
        assert_eq!(ok("a & !b").to_string(), "a and not b");
        assert_eq!(ok("-#printer").to_string(), "not +#printer");
        assert_eq!(ok("no:assignee").to_string(), "not has:assignee");
        assert_eq!(ok("tag:@work,@home").to_string(), "+@work or +@home");
        assert_eq!(parse("").unwrap().to_string(), "");
    }

    #[test]
    fn ast_survives_serde() {
        let expr = ok("(+@work or title:cold) and due<=+7d and not is:blocked");
        let json = serde_json::to_string(&expr).unwrap();
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(back, expr);
    }
}
