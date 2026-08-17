# Changelog

## Unreleased — the filter expression language

Filtering is now an expression language rather than a fixed set of tokens.
`next list`, `next next`, `next tree` and `next forecast` all take it, the TUI
filter bar evaluates it as you type, and the MCP tools take the identical
string.

### Breaking: a bare word now searches instead of selecting a tag

```sh
next list bug     # BEFORE: tasks tagged `bug`
                  # NOW:    tasks whose text mentions "bug"
next list +bug    # tasks tagged `bug`
```

Every sigil form is unchanged — `+tag`, `-tag`, `parent:`, `context:`, `user:`
all mean exactly what they did. Only the *bare* word changed meaning.

This ships without a compatibility warning, deliberately: there is no "did you
mean +bug?" nag, because a bare word is now a legitimate query in its own right
and a tool that second-guessed it would be wrong most of the time. The catch is
that the two readings often return the same tasks, which makes the change easy
to miss. If a filter starts returning something unexpected:

```sh
next list --explain bug
```

names every term it read as a search and shows the `+bug` spelling.

### Breaking: MCP `filter_tokens` and `context` are removed

Both are replaced by one `filter` string, the same syntax the CLI takes:

```jsonc
// BEFORE
{ "filter_tokens": ["+@work", "-bug"], "context": ["@work"] }
// NOW
{ "filter": "+@work -bug" }
```

Calling either old parameter fails with an error naming the replacement rather
than a generic rejection — an ignored `filter_tokens` would read as "no filter"
and return the whole list looking successful, which is worse than an error.
Agent configurations need updating.

### New

- **Full-text search.** A bare word, `"a quoted phrase"`, or `prefix*`, over
  title, description, notes and url. Matching is by whole word, so `arch` does
  not match `archive` — use `arch*`. Backed by an index that also reaches
  archived tasks, including segments pruned out of the working tree entirely.
- **Field predicates**: `status:open`, `priority>=medium`, `due<+7d`,
  `due:2026-08-01..eom`, `assignee:alice`, `slug:x`, `data.key:value`, plus
  `has:field` / `no:field` and `is:overdue|blocked|project|recurring|closed|assigned`.
- **`id:`** selects by task id — `id:a1b2c3d4`, or the full UUID with or
  without hyphens. It matches by prefix, so the eight characters a listing
  prints are enough (four is the minimum). Case is ignored, and a value that
  could not begin a UUID is refused at parse time rather than left to match
  nothing.
- **Booleans**: `and`, `or`, `not` with parentheses, also spelled `&`, `|`, `!`.
  Adjacency means `and`; precedence is `not` > `and` > `or`.
- **Dates**: ISO, offsets (`+7d`, `-2w`, `+3m`, `-1y`), `today`/`tomorrow`/
  `yesterday`, `eow`/`eom`/`eoy`, and natural language when quoted
  (`due:"next monday"`).
- **`next list --archived` takes the whole grammar**, not just tags.
- **`--count`** prints how many tasks match — the total across all pages, not
  the size of one — and **`--format table|json`** replaces `--json`, which
  stays as an alias.
- **`next tree` takes a filter**, keeping the ancestors of any match so the
  tree still has branches.
- **`next list --explain`** shows what a query parsed to, which terms were read
  as searches, and how many tasks survived each step.

### Refined after review

A pass over the filtering and projection surface, mostly about what the
commands *say*. A wrong answer gets reported; a confusing message gets lived
with, so these were the defects least likely to arrive as bug reports.

- **A flag typed after the filter is refused, and told where to go.** The
  trailing filter takes its tokens verbatim, so `next list +@work --all` fed
  `--all` to the query as a term. It used to be reported as an unrecognised
  argument, contradicting a `--help` that lists it. The message now says the
  flag belongs before the expression. Applies to `list`, `next`, `tree`,
  `forecast` and `edit`.
- **`--fields` without `--json` is an error, not a no-op.** The table has fixed
  columns and never honoured it. `list`, `show`, `tree` and `next` now refuse
  it in the same words.
- **`--fields` on `next tree` and `next next`.** Projection belongs to JSON
  output rather than to one command, and those two pay the same payload cost.
- **`--all` no longer discards a `user:` term you typed.** `--all` widens the
  implicit gate, which includes the *stored* user scope; it never meant "and
  also ignore the scope in the query". `next list --all user:bob` was listing
  alice's finished work.
- **A query the gate contradicts explains itself.** `next list status:done`
  matches nothing because the gate removes closed tasks before the expression
  runs. The listing now says so and names `--all`. Which tasks match is
  unchanged.
- **`next list --archived` honours `list_limit`.** It used the built-in default,
  so one config gave two page sizes depending on the tier.
- **`next tree --count` is answered before the output format**, as `list` does,
  and counts matches rather than the ancestors kept to keep the tree connected.
- **Messages that were pointing the wrong way.** An unknown `field:value` now
  says to quote the token to search for it literally (a pasted URL is the usual
  cause). `parent:a,b` is reported as a set where one slug belongs, distinctly
  from a repeated `parent:`. `created` and `updated` say they come from git
  history rather than "unknown field". The `--explain` search hint fires only
  for an unscoped bare word and only suggests a tag that exists.
- **The archived `--explain` stopped inventing a candidate count.** It printed
  the match count under "candidates loaded" too. An inexact pushdown reads rows
  the store never reports back, so that number is not knowable — it says so
  instead. It also stopped suggesting `--all`, which `--archived` refuses to be
  combined with.
- **`score_breakdown` is projectable, and dropped unless named**, matching
  `score` in `list`. An unprojected response is unchanged.
- **`--explain` refuses the output flags it would ignore.** It prints prose and
  has no JSON form on purpose, so `--json`, `--fields` and `--count` had
  nothing to act on beside it and were silently losing. Whichever was meant,
  saying so beats picking one.

### Notes

- `next tree --count` and `next list --count` can differ for the same query, on
  purpose: `list` applies the implicit gate and `tree` cannot, because a tree
  without its blocked tasks and open parents has no shape left to draw.
- `is:blocked` and `is:project` return little by default: the implicit gate
  already hides blocked tasks and parents with open subtasks. Pass `--all`.
- `score` cannot be filtered on — a task's score is computed after filtering.
- `next list --archived` refuses `is:blocked`, `is:project`, `parent:`,
  `created:`, `updated:`, `context:` and `user:`, all of which need either a
  view of other tasks or git history that an archive listing does not load. It
  says so rather than ignoring them.
