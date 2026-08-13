//! Assumptions this project makes about SQLite — not tests of this project.
//!
//! Nothing here exercises `next` code. Every assertion is a property of the
//! bundled SQLite engine that the filter pushdown *depends* on, written down
//! so that a SQLite upgrade which changed one would fail loudly here instead
//! of silently changing what a user's query returns.
//!
//! Why this deserves its own file: the pushdown marks most predicates
//! **exact**, meaning the SQL result is taken as the answer and nothing
//! re-checks it in memory. An exact predicate resting on a behaviour that
//! quietly changed is the one failure mode with no other guard — the
//! differential tests compare our two paths against each other, so if SQLite
//! moved under both they could still agree and both be wrong.
//!
//! Each test names the decision that rests on it. If one fails after a
//! dependency bump, the fix is in `src/core/storage/sql_filter.rs`, not here.

use rusqlite::Connection;

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE t(id TEXT, tag TEXT, priority TEXT, slug TEXT);
         INSERT INTO t VALUES('1', '@work/x', 'high',   'alpha');
         INSERT INTO t VALUES('2', 'a_b/c',   'low',    NULL);
         INSERT INTO t VALUES('3', 'axb/c',   'medium', 'beta');",
    )
    .unwrap();
    conn
}

fn count(conn: &Connection, where_sql: &str, args: &[&str]) -> i64 {
    conn.query_row(
        &format!("SELECT count(*) FROM t WHERE {where_sql}"),
        rusqlite::params_from_iter(args.iter()),
        |r| r.get(0),
    )
    .unwrap()
}

/// Depended on by: the priority rank being written as a literal, not bound.
///
/// A bound parameter arrives as TEXT, and SQLite orders every integer before
/// every string — so `CASE … END >= '1'` is false for all three priorities.
/// This already cost one silent "no results".
#[test]
fn integers_sort_before_every_string() {
    let conn = db();
    let rank = "(CASE priority WHEN 'low' THEN 0 WHEN 'medium' THEN 1 WHEN 'high' THEN 2 END)";
    assert_eq!(count(&conn, &format!("{rank} >= 1"), &[]), 2, "literal");
    assert_eq!(
        count(&conn, &format!("{rank} >= ?"), &["1"]),
        0,
        "a bound '1' is TEXT and sorts after every integer — the trap"
    );
}

/// Depended on by: the tag descendant test using `substr`, not `LIKE`.
///
/// Both behaviours below would be over-matches the evaluator rejects, in a
/// predicate marked exact.
#[test]
fn like_folds_ascii_case_and_treats_underscore_as_a_wildcard() {
    let conn = db();
    assert_eq!(
        count(&conn, "tag LIKE ? || '/%'", &["@WORK"]),
        1,
        "LIKE is case-insensitive for ASCII"
    );
    assert_eq!(
        count(&conn, "tag LIKE ? || '/%'", &["a_b"]),
        2,
        "'_' is a LIKE wildcard and also a legal tag character"
    );
}

/// Depended on by: the replacement for `LIKE` being both case-sensitive and
/// wildcard-free.
#[test]
fn equality_and_substr_are_binary_and_wildcard_free() {
    let conn = db();
    let prefix = "substr(tag, 1, length(?) + 1) = ? || '/'";
    assert_eq!(count(&conn, prefix, &["@WORK", "@WORK"]), 0, "case matters");
    assert_eq!(count(&conn, prefix, &["@work", "@work"]), 1);
    assert_eq!(
        count(&conn, prefix, &["a_b", "a_b"]),
        1,
        "'_' is literal here, so `axb/c` is not matched"
    );
}

/// Depended on by: `negate` wrapping its operand in `COALESCE(…, 0)`.
///
/// The evaluator reads an unset field as "predicate false" and negates that to
/// TRUE, so `not slug:alpha` must return the row with no slug at all. SQL's
/// three-valued logic drops it instead.
#[test]
fn not_over_null_is_null_not_true() {
    let conn = db();
    assert_eq!(count(&conn, "NOT (slug = ?)", &["alpha"]), 1, "row 3 only");
    assert_eq!(
        count(&conn, "NOT COALESCE(slug = ?, 0)", &["alpha"]),
        2,
        "the NULL-slug row must come back too"
    );
}

/// Depended on by: marking search atoms exact at all.
///
/// The index is created `remove_diacritics 0` so it tokenises the way
/// `filter_eval::words` does — lowercase, split on non-alphanumeric, accents
/// kept. If that stopped holding, search would quietly answer differently from
/// the scan on the cacheless path.
#[test]
fn fts5_is_available_and_tokenises_the_way_the_scan_does() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE f USING fts5(
             task_id UNINDEXED, title, description, notes, url,
             tokenize = 'unicode61 remove_diacritics 0');
         INSERT INTO f VALUES('1', 'Café naïve', '日本語 テスト', 'wifi-router', '');",
    )
    .expect("the bundled SQLite must ship FTS5");

    let hits = |q: &str| -> i64 {
        conn.query_row("SELECT count(*) FROM f WHERE f MATCH ?", [q], |r| r.get(0))
            .unwrap_or(-1)
    };

    assert_eq!(hits("\"café\""), 1, "accented term matches");
    assert_eq!(hits("\"CAFÉ\""), 1, "case folds");
    assert_eq!(hits("\"cafe\""), 0, "accents are NOT folded away");
    assert_eq!(hits("\"日本語\""), 1, "CJK tokenises as our scan does");
    assert_eq!(hits("\"caf\""), 0, "a bare term is a whole word");
    assert_eq!(hits("\"caf\"*"), 1, "trailing star is the prefix form");
    assert_eq!(hits("\"wifi router\""), 1, "hyphen splits into two words");
    assert_eq!(hits("\"router wifi\""), 0, "a phrase is ordered");
}

/// Depended on by: `search_sql` rebuilding the phrase from tokens and quoting
/// it, rather than passing the user's term through.
///
/// A user searching for an FTS5 operator must get a literal — and must never
/// get a MATCH *parse error*, which would surface as a failed command instead
/// of an empty result.
#[test]
fn fts5_quoting_neutralises_query_syntax() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE f USING fts5(body, tokenize = 'unicode61 remove_diacritics 0');
         INSERT INTO f VALUES('and or not near this word');
         INSERT INTO f VALUES('nothing relevant');",
    )
    .unwrap();
    let hits = |q: &str| -> i64 {
        conn.query_row("SELECT count(*) FROM f WHERE f MATCH ?", [q], |r| r.get(0))
            .unwrap_or(-1)
    };

    for term in ["\"and\"", "\"or\"", "\"not\"", "\"near\""] {
        assert_eq!(hits(term), 1, "{term} must be a literal, not an operator");
    }
    // The column-filter form `search_sql` emits for `title:foo`.
    assert_eq!(hits("{body} : \"word\""), 1);

    // Raw user text CAN be a MATCH syntax error — an unbalanced quote or a
    // bare `*` both fail to parse. That is the failure quoting prevents: our
    // builder emits tokens of alphanumerics inside one balanced pair, so it
    // cannot construct either of these however strange the search term is.
    assert_eq!(hits("\""), -1, "an unbalanced quote is a syntax error");
    assert_eq!(hits("*"), -1, "a bare star is a syntax error");

    // An unquoted operator BETWEEN terms is read as syntax, not as a word:
    // `and` here joins two terms rather than matching the literal "and".
    assert_eq!(
        hits("word AND relevant"),
        0,
        "AND joins, so neither row has both"
    );
    assert_eq!(hits("\"word\" OR \"relevant\""), 2, "OR joins both rows");
}
