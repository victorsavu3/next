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

### Notes

- `is:blocked` and `is:project` return little by default: the implicit gate
  already hides blocked tasks and parents with open subtasks. Pass `--all`.
- `score` cannot be filtered on — a task's score is computed after filtering.
- `next list --archived` refuses `is:blocked`, `is:project`, `parent:`,
  `created:`, `updated:`, `context:` and `user:`, all of which need either a
  view of other tasks or git history that an archive listing does not load. It
  says so rather than ignoring them.
