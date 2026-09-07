# Changelog

## Unreleased

### New

- **Snap leeway: a snap boundary is now a tolerance, not a ratchet.** Set up a
  rent task the way the docs suggested — `--recur-completion 30 --recur-snap
  dom:1` — complete it on the 2nd instead of the 1st, and the next instance was
  due sixty days later rather than thirty. The 1st had gone past, and a snap
  only ever moved a date *forward*, so it jumped to the month after. Do that
  every month, as people actually do, and a task configured to fire twelve
  times a year fires seven. Nothing warned you, and nothing in `next show` let
  you see why.

  `--recur-snap-leeway <SPEC>` bounds how far the snap may move the date, and
  makes it two-sided:

  ```sh
  next add "Pay rent" --slug rent --due 2026-06-01 \
    --recur-completion 30 --recur-snap dom:1 --recur-snap-leeway 3

  next edit rent --recur-snap-leeway 5,0   # pull back up to 5 days, never push later
  ```

  `N` sets both directions, `BACK,FORWARD` sets each, in whole days from 0 to
  365, and `BACK,*` leaves the forward direction unbounded. The rule is one
  sentence: if a qualifying boundary falls within the
  tolerance of the computed date, move to it — nearest wins, ties go forward —
  and otherwise **keep the computed date unsnapped**.

  | Completed | Before | With `--recur-snap-leeway 3` |
  |-----------|--------|------------------------------|
  | 2026-06-01 | 2026-07-01 | 2026-07-01 |
  | 2026-06-02 | 2026-08-01 | 2026-07-01 |
  | 2026-06-15 | 2026-08-01 | 2026-07-15 |

  That last row is the part worth dwelling on, because it looks like the
  feature failing. With a leeway of 3 the 1st is out of reach from the 15th, so
  the date stays on the 15th — off the boundary, but on cadence. Preserving the
  interval is the property being bought; landing on the boundary was only ever
  a means to it. Snapping anyway would reintroduce the unbounded move that
  caused the bug.

  The same reasoning has a consequence you should know before typing a small
  number. `next-workday` sits one day back from Friday and two days forward to
  Monday when the computed date is a Saturday, and the mirror for a Sunday. So
  a Saturday only reaches Monday if the forward tolerance is at least 2, and a
  Sunday only reaches Friday if the backward one is. With `--recur-snap-leeway
  0,1` a Saturday simply stands. That is the rule working, but it will read as
  a bug, so `next show` now prints the leeway next to the snap.

  **Nothing you already have changes.** An absent leeway means back 0, forward
  unbounded — precisely the old behaviour — so no stored task moves a date, no
  file is rewritten, and a repository synced between an upgraded and a
  non-upgraded machine agrees on every date it already had. There is no
  migration step. The cost of that choice is that the default is still the
  ratchet, so `next add` prints a hint when you set a `dom:N` or weekday snap
  without a leeway, rather than silently deciding for you.

  It applies to both recurrence types, reaches `next add`, `next edit`, the MCP
  `add_task` / `update_task` tools and the TUI edit form, and is stored as a
  `[recurrence.snap_leeway]` table beside `[recurrence.snap]`.

  On a schedule (`RRULE`) rule each occurrence is spent once. A backward
  tolerance can pull a Monday onto the Friday before it, and the next `next
  done` steps over that Monday rather than offering it again, so a weekly rule
  still fires weekly however wide the tolerance. `next forecast` walks the same
  series, so what it shows is what `next done` will produce.

- **Five recurrence fixes that the leeway work depended on**, each a bug in its
  own right. The changelog has been silent about recurrence entirely up to now,
  so they are worth stating rather than folding into the entry above.

  `--recur-completion 0` was accepted. A zero-day interval never advances: every
  spawned instance is due the day it is created, forever. It is now rejected
  where the rule is built, alongside the `INTERVAL=0` check RRULEs already had.

  `next edit <id> --recur-completion N` silently destroyed the task's snap.
  Changing the interval on a snapped task meant re-typing `--recur-snap` or
  losing it without being told. The snap — and now the leeway — is carried
  forward across a rule change; `--clear-recur-snap` is the way to drop it, and
  it drops the leeway with it, since a tolerance without a boundary means
  nothing.

  The forecast emitted duplicate dates. A snap maps whole runs of computed
  occurrences onto the same boundary — a daily rule snapped to Monday hits that
  Monday five times — and every one of them was reported. Each date is now
  reported once, and the per-series cap counts dates emitted rather than steps
  taken, so a snapped series reaches as far into the horizon as an unsnapped
  one.

  The completion-mode forecast walked the wrong series. It documented an
  assumption — that each instance is completed on its due date — and then
  advanced on the *un-snapped* date, which under a snap is a different date
  from the one it had just shown you. The projection drifted off the boundary
  and ran a period behind what completing on the due date actually produces.
  It now steps from the date it emitted, so the forecast and `next done` agree.

  `next show` did not display the recurrence configuration. It printed the rule
  and dropped the snap and the anchor, so there was no way to confirm what you
  had set — or to notice that the edit above had just deleted it. It now shows
  the snap and its leeway, including the case where no leeway is set, since an
  invisible default is what made the original bug so hard to see.

- **Progress reporting.** The operations that used to look like a hang — the
  cache rebuild on a fresh clone, the archive pass, a fetch or push, a tag
  rename across every tier — now draw a spinner or a bar while they run.

  That includes the rebuild no command asks for: opening a repository whose
  cache is missing (a fresh clone) or stale (a manual `git pull`) rebuilds it
  first, and now says so, whatever command you actually ran.

  It is drawn on **stderr** and cleared when the phase ends, so results on
  stdout are untouched and nothing is left on screen afterwards. Nothing
  appears for the first 100 ms, so the operations that finish instantly (most
  of them) stay invisible — but the wait on an unreachable remote does appear,
  because the delay is a timer rather than a "show it once something happens".

  Progress is drawn only when stderr is a terminal, `TERM` is not `dumb`, the
  command is not printing JSON, and neither of the two new global flags was
  given:

  - `--no-progress` — never draw progress.
  - `--quiet` / `-q` — no progress **and** no informational notes on stderr
    (`note: pulled latest changes…`, `warning: auto-pull failed…`, and the
    `Synced with remote.` confirmation). Errors and command results are
    unaffected.

  `NEXT_FORCE_PROGRESS=1` renders anyway — through a redirected stderr, for a
  JSON command, on a dumb terminal — and skips the 100 ms delay so a forced run
  always paints. It does not override `--quiet` or `--no-progress`.

  `next-mcp`, `next-forgejo` and library consumers render nothing, by
  construction: the CLI is the only surface that installs a renderer.

## 1.5.0 — the filter expression language

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
