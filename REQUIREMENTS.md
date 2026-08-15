# Requirements

Precision level: MUST = mandatory, SHOULD = strongly preferred, MAY = optional.

---

## 1. Data model

### 1.1 Task fields

Each task MUST carry the following fields:

| Field | Type | Notes |
|-------|------|-------|
| `id` | UUID v4 string | Assigned on creation, never changed |
| `title` | non-empty string | |
| `status` | `open` \| `started` \| `done` \| `cancelled` | |
| `created_at` | RFC 3339 datetime | Set on creation |
| `updated_at` | RFC 3339 datetime | Updated on every write |

A task MAY carry the following optional fields:

| Field | Type | Notes |
|-------|------|-------|
| `priority` | `low` \| `medium` \| `high` | Default: `medium` |
| `due` | date string (YYYY-MM-DD) | Optional deadline |
| `start` | date string (YYYY-MM-DD) | Task hidden until this date; disables age scoring |
| `long_term` | boolean | Disables age scoring; default `false` |
| `slug` | string | User-provided identifier (e.g. `"water-plants"`); must be unique |
| `parent_id` | UUID string | UUID of the parent task; used for subtasks and project membership |
| `tags` | array of strings | See §3 |
| `blocked_by` | array of UUID strings | Explicit blockers |
| `score_adjustment` | float | Added directly to computed score |
| `assignee` | string | Username of the person responsible for the task |
| `description` | string | Multi-line free-form description providing context beyond the title |
| `url` | string | URL associated with the task (ticket, doc, reference link); must be http/https |
| `notes` | string | Multi-line free text |
| `data` | map of string → JSON value | Arbitrary key-value pairs for tool integrations or AI-provided metadata; values may be any JSON type except null |
| `completed_at` | date string (YYYY-MM-DD) | Date the task was completed; set when marked done (defaults to today, may be backdated via `--completed-at`). Absent while unresolved |

When a task recurs it MUST carry a `[recurrence]` table. The `type` field selects the mode:

```toml
# Schedule-based: next instance follows a fixed calendar rule
[recurrence]
type = "schedule"
rrule = "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"   # RFC 5545 RRULE string (no prefix)
anchor = "2026-05-26"                          # first instance date; pins interval alignment

# Completion-based: next instance is N days after completion
[recurrence]
type = "completion"
interval_days = 7

# Optional snap on either type: round the computed date forward to a boundary
[recurrence.snap]
type = "next_weekday"   # "next_weekday" | "next_workday" | "day_of_month"
weekday = 5             # 0=Mon…6=Sun; used with next_weekday
```

Supported RRULE fields: `FREQ` (`DAILY`, `WEEKLY`, `MONTHLY`, `YEARLY`), `INTERVAL`, `BYDAY`, `BYMONTHDAY`.

### 1.2 Projects and subtasks

There is no separate project type. Any task can have child tasks by setting `parent_id`
on the children. Hierarchy nests to unlimited depth. A task that has children is a
project — no special tag or field is required to declare it as such.

Use `next tree` to see the full parent-child hierarchy and `next show` to inspect a
single task and its direct children. Use `parent:<slug>` as a filter token to list
tasks within a specific subtree (see §5).

A task's slug (e.g. `"work-infra"`) can be used instead of its UUID when specifying
`parent_id` or `blocked_by` on the command line. The tool resolves the slug to a UUID
before writing the TOML file.

### 1.3 Global state

Machine-local state (the per-tag state and the active users) MUST be stored
at `$XDG_STATE_HOME/next/<fnv1a-hash-of-repo-path>/state.toml`.  This path is
never inside the repository and MUST NOT be committed to git.

```toml
active_users = ["alice"]            # active user filter (empty = no filter)

[tags]                              # one entry per tag with a state; absent = inherit
"@home"          = "required"
"#printer"       = "excluded"
"@home/kitchen"  = "accepted"       # pinned: ignores any state on @home
```

The tag map MUST be keyed by the full tag including its sigil, so every kind of tag lives
in one namespace and no bare-name conversion is needed.

Tag descriptions are human-readable notes attached to any tag (context, resource, or
freeform). They are stored as individual TOML files under `tags/` in the repository
(e.g. `tags/__context__work.toml`, `tags/__context__home/kitchen.toml`) using the
`__context__`/`__resource__` encoding, and ARE committed to git so that all machines
share the same descriptions. The `next tag describe` command writes these files.

---

## 2. Storage

### 2.1 Repository layout

```
<repo-root>/
  tasks/
    water-plants-a1b2c3d4.toml   # filename: <slug>.toml if slug set, else <title-slug>-<first-8-uuid>.toml
    work-infra.toml              # project task with slug "work-infra"
    deploy-db-e5f6a7b8.toml
  archive/
    2025/
      10-001.toml                # warm-tier segment: ≤1000 archived tasks of one completion month
      10-002.toml
    pruned.jsonl                 # cold-tier manifest: pruned segments (path + blob SHA), union-merged
  tags/
    __context__work.toml         # tag description for @work  (@ → __context__)
    __context__home/
      kitchen.toml               # tag description for @home/kitchen
    __resource__printer.toml     # tag description for #printer  (# → __resource__)
  config/
    scoring.toml                 # committed scoring weights (seeded by `next init`)
    archive.toml                 # committed archive policy (optional; absent = defaults)
  .gitignore                     # MUST contain ".next.db" (and the other generated files, §8.0)
  .next.db                       # SQLite read cache; MUST NOT be committed to git

$XDG_STATE_HOME/next/<repo-hash>/
  state.toml                     # machine-local state; MUST NOT be committed to git
```

All *active-tier* task files MUST reside in the flat `tasks/` directory. There is no
`projects/` directory; project tasks are stored alongside all other tasks. Archived
tasks live in `archive/` segments (see §2.3).

File names MUST follow this rule: if the task has a `slug`, the file is named
`<slug>.toml`; otherwise `<title-slug>-<first-8-uuid>.toml` where the title slug is
lower-case with spaces replaced by `-` and non-alphanumeric characters stripped.
Example: `"Water plants"` with no slug and UUID `a1b2c3d4-…` → `water-plants-a1b2c3d4.toml`.

### 2.2 Sync

`next sync` MUST execute the following steps in order, stopping on any error:

1. `git pull` from the configured remote (fast-forward or merge)
2. If merge conflicts exist, print an actionable error message and exit with code 2
3. Run the automatic archive pass if it is due (see §2.3; at most once per day)
4. `git push` local commits to the remote

All mutations (add, edit, done) MUST produce a git commit automatically. The
commit message MUST identify the operation and the task title.

### 2.3 Archiving

The design targets one million tasks with these budgets: task edits < 100 ms,
queries < 1 s, incremental cache reconciliation of ≤ 200 changes < 10 s, and a
full cache rebuild < 10 min (verified by `cargo bench --bench large_repo`).
Tasks move through three storage tiers:

* **Active** — one TOML file per task under `tasks/`. Open and recently
  closed tasks.
* **Warm** — closed tasks whose reference date (`completed_at`, else the
  git-derived last-update time) is older than `archive_after_days` move into
  append-once segment files `archive/<YYYY>/<MM>-<NNN>.toml`, keyed by
  completion month and sealed at `segment_max_tasks` entries. Each entry is
  the full task plus its git-derived creation/update timestamps frozen at
  archive time, so the archive never needs git history. Segment bytes MUST be
  deterministic (entries sorted by `(completed_at, id)`) so concurrent passes
  on different machines merge silently.
* **Cold** (optional) — segments whose newest completion is older than
  `prune_after_days` leave the checkout entirely. The append-only manifest
  `archive/pruned.jsonl` records each pruned segment's path and blob SHA
  (union-merged; the last line per path wins); recovery is a single blob
  read, never a history walk.

Policy lives in the committed `config/archive.toml`:

```toml
archive_after_days = 180    # warm threshold (default)
segment_max_tasks  = 1000   # segment seal size (default)
auto               = true   # run the pass automatically during sync (default)
# prune_after_days = 730    # cold threshold; absent = pruning disabled (default)
```

Eligibility guards: a closed task still carrying a `recurrence` rule MUST NOT
archive while it is the newest instance of its series, and a parent MUST NOT
archive while any active-tier child is ineligible (subtrees archive bottom-up).

The automatic pass runs during sync (post-pull, pre-push), at most once per
day per machine (`last_archive` in the machine-local state); `next maintenance
archive` runs it on demand without the throttle. Archive/prune failures MUST NOT
fail the sync.

**Resurrection**: reads never resurrect. Any *mutation* that resolves to an
archived task MUST first move it back to the active tier inside the same
transaction — out of its segment (fetched from its blob when cold), back to
`tasks/`, with the frozen creation date preserved — and the touched segment
is committed together with the mutation. A resurrected task re-archives on a
later pass if it becomes eligible again. Deleting an archived task is an
error (resurrect first). Direct lookups (id, slug, UUID prefix) MUST work
across all tiers; `list` and scored views serve the active tier only, and
`--archived` serves the archive.

---

## 3. Tag system

Tags are strings in the `tags` array of a task. The prefix is a **naming convention**
that says what a tag is for; it MUST NOT change how the tag is filtered:

| Prefix | Kind | Example | Usually names |
|--------|------|---------|---------------|
| `@` | Context | `@home`, `@work` | A working environment |
| `#` | Resource | `#printer`, `#vacation` | Something that must be available |
| *(none)* | Freeform | `python`, `reading`, `project` | Anything else |

The kinds MAY be used to group tags for display (the tag catalogue does), and MAY be
queried (`has:context`), but MUST NOT be given distinct filtering rules.

### 3.1 Tag filtering

All filtering by tag is governed by the tag state in §3.1.1. When no tag has a state, all
tasks MUST be shown regardless of their tags.

A query-time `context:@name` filter MUST replace the *required* set for that single
invocation, leaving exclusions in force.

### 3.1.1 Tag state

Every tag MUST take the same three states, whatever its sigil: `required`, `excluded`, or
`accepted`. `@` and `#` are naming conventions and MUST NOT change how a tag filters — in
particular a `#resource` is not restricted to being excluded, and an `@context` gets no
special treatment.

Two rules, applied to every tag alike:

1. a task carrying any tag that resolves to `excluded` MUST be hidden;
2. while any tag is `required`, a task MUST carry at least one tag resolving to
   `required` to be shown. Required tags are a disjunction.

Rule 1 takes precedence over rule 2. A task carrying no tags at all is therefore hidden
whenever anything is required.

**Inheritance.** A tag with no entry inherits the state of its nearest ancestor that has
one, so excluding `#office` also excludes `#office/printer`. The most specific entry
wins. An explicit `accepted` MUST stop that inheritance, which is the only way for a
child to opt out of its parent's state. Matching is downward only: requiring `@work`
covers `@work/frontend`, but requiring `@work/frontend` does NOT cover `@work`.

**Independence from `--all`.** The tag state MUST still apply when the implicit gate is
disabled: `--all` widens the statuses shown, not the tags.

**Reading a state this build does not know.** An unrecognised value in the `[tags]` map
MUST be dropped rather than fail the load. `state.toml` is read on every command, so a
value written by a newer build must not make the tool unusable. The previous names
(`included`, `default`) MUST still load as `required` and `accepted`, so a rename does
not silently discard a user's state; nothing writes them.

CLI: `next tag require|exclude|accept <tag>...` / `next tag clear-state [<tag>...]`
MCP: `set_tag_state` with `tags` and `state` (`required`/`excluded`/`accepted`/`clear`).

### 3.3 User filtering

When one or more usernames are present in `active_users`:
- Tasks with **no** `assignee` MUST always be included (unassigned = shared backlog)
- Tasks whose `assignee` matches any name in `active_users` MUST be included
- Tasks whose `assignee` is set to a name **not** in `active_users` MUST be excluded

When `active_users` is empty, all tasks MUST be shown regardless of their `assignee`.

A query-time `user:<name>` filter token MUST override the global active-user set for that
single invocation. The `--all-users` flag MUST bypass the user filter entirely for that
invocation.

---

## 4. Scoring

Every task that passes filtering MUST be assigned a numeric urgency score used to rank
the default output.

A task whose `status` is `done` or `cancelled` MUST score exactly `0.0`, with every factor
zeroed — urgency ranks what to work on next, which is meaningless for a finished task. The
rule MUST be enforced in the scoring core so that every surface that displays a score
(`next list --closed` / `--all`, `next show`, the TUI, the MCP tools) agrees. A consequence
is that scoring no longer reorders a closed listing: it comes out in the order the store
produced it (most recent completion first, ties by id), matching `next list --archived`.

For any other task, the score MUST be the sum of the following weighted factors:

| Factor | Condition |
|--------|-----------|
| **Due proximity** | Zeroed when any tag has `no_time_urgency = true`; otherwise rises as due date approaches |
| **Priority** | Always; `low` / `medium` / `high` map to fixed additive weights |
| **Project factor** | When `parent_id` is set; parent task's priority contributes an offset |
| **Age** | Zeroed when any tag has `no_time_urgency = true`, or when `long_term = true`, or when `start > today` |
| **Tag factor** | Sum of priority offsets for each of the task's tags that carry explicit `priority` metadata; tags with no priority metadata contribute `0.0` |
| **Parent tag factor** | Same as tag factor, but applied to the parent task's tags (when a parent exists) |
| **Started bonus** | Flat additive bonus when `status == started` |
| **User adjustment** | Always; `score_adjustment` added directly |

Default weights MUST be defined in code and SHOULD be overridable via a config file
committed to the repository at `config/scoring.toml`. Because the weights live in the
repository (not in machine-local config), every consumer — the CLI, the MCP server, and
plugins — MUST share the same scoring view. `next init` seeds the file with the defaults;
absent or partial files fall back to the built-in defaults.

Tasks excluded from the default view (blocked, hidden by the tag state, future `start`,
parent awaiting subtasks) MUST NOT receive a score and MUST NOT appear in `next list` /
`next next` output unless `--all` is passed. Note that `--all` does not lift the tag
state (§3.1.1).

---

## 5. Filtering and queries

Filtering MUST be an expression language, and every surface MUST parse the same
string with the same code: the CLI (`list`, `next`, `tree`, `forecast`, joining
its trailing arguments), the TUI filter bar, and the MCP `filter` parameter. A
query MUST be transferable between them unchanged — that is the property the
single parser exists to guarantee, not merely a convenience.

The grammar MUST provide: full-text search as the DEFAULT atom (a bare word,
a quoted phrase, a `prefix*`), tags behind a sigil (`+tag` requires, `-tag`
excludes, both matching nested tags), field predicates with `:` and the ordered
operators `< <= > >=` and `low..high`, `has:`/`no:`, the named `is:` predicates,
and the booleans `and`/`or`/`not` with parentheses in both word and symbol
spellings. Adjacency MUST mean `and`, and precedence MUST run `not` > `and` >
`or`.

A bare word MUST search rather than select a tag. The two readings often return
the same tasks, so `next list --explain` MUST name which terms were read as
searches; there is deliberately no in-tool warning.

Parser recursion MUST be bounded. Unbounded recursion on a nested expression
overflows the stack, which aborts the process rather than raising a reportable
error — on a long-running MCP server that would take down every session.

An unparseable query MUST be refused with an error naming the cause. It MUST
NOT be silently reinterpreted, and a surface MUST NOT fall back to "no filter",
which would return everything while looking successful.

### 5.1 Filter tokens

All list commands MUST accept the following, freely combinable:

| Syntax | Meaning |
|--------|---------|
| `+<tag>` | Task must have this tag |
| `-<tag>` | Task must not have this tag |
| `parent:<slug>` | Task is a descendant of (or is) the task with this slug |
| `context:<@tag>` | Use this context instead of the active set for this query |
| `user:<name>` | Use this user instead of the active-user set for this query |
| `--future` | Include tasks with future `start` date and planned recurrence instances |
| `--all` | Disable all implicit filtering (contexts, resources, blocked, start date) |
| `--all-users` | Bypass the user filter for this query |

Multiple `+tag` tokens MUST be combined with AND (task must have all of them).
Multiple `-tag` tokens MUST be combined with AND (task must have none of them).

---

## 6. Blocking and subtasks

### 6.1 Explicit blockers

A task with non-empty `blocked_by` MUST be excluded from the default list and scoring
until every referenced task has `status = done` or `status = cancelled`.

### 6.2 Subtasks

Any task can have child tasks via `parent_id`. A parent task is excluded from the
*default scored list* until all of its direct children have `status = done` or
`status = cancelled`; it does not appear in `next list` / `next next` output until then.

For both §6.1 and §6.2, `status = started` counts as active (same as `open`) — a started
child still blocks its parent, and a started task still blocks tasks that list it in
`blocked_by`.

The user MAY mark a parent task done at any time via `next done <id>` regardless of
child task status — the completion gate only affects automatic scoring visibility, not
explicit user actions.

A parent task MUST NOT be automatically marked `done` when all subtasks complete; the
user marks it done explicitly.

Blocking is **not** transitive through depth: a grandparent is only blocked by its
direct children (who are in turn blocked by their own children).

Project tasks that are long-running SHOULD use `long_term = true` to suppress age-based
scoring while the project is in progress.

---

## 7. Recurrence

`next done` on a recurring task marks it done AND creates the next instance atomically in a single git commit. Cancelling a recurring task does NOT spawn a next instance.

### 7.1 Completion-based

1. Current task is marked `done`
2. Next task is created with `start` (or `due`) = `today + interval_days`
3. If a snap is set, the computed date is advanced to the nearest qualifying boundary
4. If the original task has both `start` and `due`, the offset between them is preserved

### 7.2 Schedule-based

1. Current task is marked `done`
2. `after = max(task.due, task.start, today)` — never re-uses a date already passed
3. The RRULE is evaluated from `anchor` to find the first occurrence strictly after `after`
4. If a snap is set, the resulting date is advanced further
5. If the original task has both `start` and `due`, the same offset is applied to the new occurrence

The `anchor` is set once (on `next add`) to the task's `start` or `due` date, falling back to today. All future instances carry the same `anchor` so INTERVAL calculations stay aligned.

For `MONTHLY`/`YEARLY` rules, a target day that does not exist in a given month is CLAMPED to that month's last day rather than skipping the month/year: the 31st becomes the month's last day (e.g. Apr 30, Feb 28), and Feb 29 becomes Feb 28 in non-leap years. When several `BYMONTHDAY` values clamp to the same date (e.g. 30 and 31 both → Feb 28), the occurrence is counted once.

A schedule RRULE MUST be validated when it is set (on `next add`/`next edit`, and via the MCP `add_task`/`update_task` tools): the rule is parsed with the same parser used to compute occurrences, and an invalid or unsupported rule (missing `FREQ`, `INTERVAL` < 1, non-positive `BYMONTHDAY`, unknown `FREQ`, positional `BYDAY`, etc.) is rejected with a clear error at set time. A malformed rule MUST NOT be stored and MUST NOT be deferred to fail later on `next done`.

### 7.3 Snap values

After computing the raw next date, an optional snap advances it to a boundary:

| Snap type | TOML | Description |
|-----------|------|-------------|
| Next weekday | `type = "next_weekday"; weekday = N` | 0=Mon…6=Sun; keep the date if already there |
| Next workday | `type = "next_workday"` | Advance to the next Mon–Fri |
| Day of month | `type = "day_of_month"; day = N` | Day 1–28; use current month if not yet passed, else next |

### 7.4 Series identity

All instances of a series share the same `recurrence_id` UUID (equal to the first instance's `id`). Slugs are not propagated to spawned instances.

`next forecast` MUST accept the same filter tokens as `next list` and MUST display the
upcoming due dates for all matching recurrence series over a configurable horizon
(`forecast_horizon_days`, default 90, overridable with `--days`).

For each active (open/started) schedule-type recurring task, the forecast MUST project
the series forward: starting after the current instance's date (`max(due, start, today)`),
it repeatedly evaluates the RRULE (`next_occurrence`, then any snap) to enumerate the
successive occurrences up to and including `today + horizon`. These projected,
not-yet-spawned occurrences MUST be shown distinctly from concrete existing tasks (a
`(projected)` marker in text output; a `projected: true` flag in `--json`).

Completion-type recurrence is NOT projected: its next date is `completion_date +
interval_days`, and future completion dates are unknown, so only the current instance is
shown. Done/cancelled recurring tasks are not projected. Non-recurring tasks with a due
date within the horizon appear unchanged.

This projection logic lives in a single shared core helper (`recurrence::project_series`)
used by both `next forecast` and the MCP `get_forecast` tool (§12.5), so the two
implementations cannot drift.

---

## 8. CLI commands

The binary MUST be named `next`. All commands MUST support `--json` to emit JSON output.
Exit codes: `0` success, `1` user/input error, `2` system error.

### 8.0 `next init`

```
next init
```

Initialises a new task repository in the current directory:

1. Runs `git init` if no `.git` directory exists (idempotent on existing repos)
2. Creates the `tasks/` directory if it does not exist
3. Writes `config/scoring.toml` with the default scoring weights if absent
4. Appends the generated-file entries (`.next.db`, its WAL sidecars `.next.db-wal` /
   `.next.db-shm`, `.next.lock`, `state.toml`) to `.gitignore` (creates the file if
   absent; does not duplicate entries)
5. Attempts an initial git commit; skips silently if git user is not configured

MUST be safe to run more than once — subsequent runs MUST NOT corrupt existing data or duplicate `.gitignore` entries.

### 8.1 `next add`

```
next add <title> [options]
```

| Option | Notes |
|--------|-------|
| `--due <expr>` | Natural-language date accepted ("in two weeks", "next Monday") |
| `--start <expr>` | Natural-language date accepted |
| `--priority high\|medium\|low` | |
| `--slug <slug>` | User-provided identifier; must be unique; used as the filename |
| `--tag <tag>` | Repeatable |
| `--parent <id-or-slug>` | Sets `parent_id`; accepts UUID, UUID prefix, or slug |
| `--blocked-by <id-or-slug>` | Repeatable; accepts UUID, UUID prefix, or slug |
| `--description <text>` | Multi-line description providing context beyond the title |
| `--url <url>` | URL associated with this task (must be http or https) |
| `--notes <text>` | Free-text notes |
| `--recur-schedule <rule>` | Creates a schedule-based recurring task; `rule` is an RRULE string |
| `--recur-completion <days>` | Creates a completion-based recurring task |
| `--recur-snap <snap>` | Optional snap applied after the next-date computation; see §7.3 |
| `--long-term` | Sets `long_term = true` |
| `--adjust <float>` | Sets `score_adjustment` |
| `--assignee <name>` | Sets `assignee` |

When any `@context` tag is required and the new task carries no `@context` tags, those
required contexts MUST be automatically appended to the task's `tags` array. If the user
supplies any `@` tag, auto-apply is skipped. Only contexts are inherited this way: a
required `#resource` or freeform tag MUST NOT be attached to a new task, since unlike a
working environment it is not implied by where the task was captured.

### 8.2 `next list` and `next next`

```
next list [filters...]    # full scored list; overdue/due-today shown first with emphasis
next next [N] [filters...]  # top N tasks by score (default N=10)
```

Both commands MUST show overdue and due-today tasks visually distinct (e.g. coloured or
prefixed) at the top of the output. Both MUST accept `--all-users` to bypass the user
filter. Running `next` with no subcommand MUST behave as `next list`.

`next list` output is **paginated**: `--page-size` (or its shorthand `-n`/`--limit`,
or `list_limit` in config) sets the window size — default 50 — and `--page` selects
the 1-indexed page. Text output MUST indicate when the result is a window on a larger
set; `--json` returns `{ items, page, page_size, total }`. Additional list modes:
`--closed` shows only done/cancelled active-tier tasks, and `--archived` lists archived
tasks (most recently completed first; scoring and the implicit gate do not apply).
`--archived` conflicts with `--all`, `--closed`, and `--future`.

The archived tier MUST accept the same filter grammar as the active tier, over both
warm segments and pruned cold ones. The terms it cannot answer — `is:blocked`,
`is:project` and `parent:` (questions about other tasks), `created:` and `updated:`
(git history), `context:` and `user:` (whole-view scoping) — MUST be refused with an
error naming the reason, never silently ignored: a query that quietly drops half its
meaning returns a plausible wrong answer.

### 8.3 Task actions

```
next show <id-or-slug>             # full task details including subtasks, blockers, and score breakdown
next start <id-or-slug>            # mark as started (in-progress); logs a time entry
next stop <id-or-slug>             # stop a started task (returns to open); logs a time entry
next done <id-or-slug> [--completed-at <date>]  # mark done; triggers recurrence if applicable
next cancel <id-or-slug>           # mark cancelled
next edit <id-or-slug> [options]   # modify fields (same options as add, plus --clear-* flags)
next delete <id-or-slug>           # permanently remove (prompts for confirmation; --yes to skip)
next move <id-or-slug> --parent <id-or-slug>  # change the parent task
next open <id-or-slug>             # open the task's URL in the default browser
```

`next start` and `next stop` append entries to `data["time_log"]` (an array of
`{event: "start"|"stop", at: <RFC 3339 timestamp>}` objects). These entries accumulate
across multiple start/stop cycles and can be used for time-tracking analysis.

`started` tasks pass all implicit filters — they appear in `next list` / `next next`
alongside `open` tasks and count as active for blocking and parent-child visibility checks.

All `<id-or-slug>` arguments MUST accept a full UUID, an unambiguous UUID prefix
(minimum 4 hex characters), or a task's slug.

`next done` MUST record the completion date in the task's `completed_at` field. The date
defaults to today; `--completed-at <date>` overrides it. The same date is the base date for
recurrence scheduling (completion-based: `completed_at + interval_days`; schedule-based:
`max(task.due, task.start, completed_at)`). Accepts ISO 8601 or natural-language dates.

`next open` MUST fail with an error when the task has no `url` field set.

### 8.4 Tag state management

One set of commands for every kind of tag (see §3.1.1); there are no context-specific or
resource-specific commands.

```
next tag                                 # show the current state, then the tag catalogue
next tag require <tag>...                # only these tags' tasks are listed
next tag exclude <tag>...                # hide tasks carrying these tags
next tag accept <tag>...                 # pin to accepted, ignoring a parent tag's state
next tag clear-state [<tag>...]          # drop entries (all of them when none given)
```

### 8.5 Tag metadata

```
next tag                                          # list all tags grouped by kind
next tag rename <old> <new> [--merge]             # rename a tag and everything nested under it
next tag describe <tag> <text>                    # set description (any tag kind)
next tag clear-description <tag>                  # remove description
next tag set-url <tag> <url>                      # attach a reference URL
next tag clear-url <tag>
next tag set-priority <tag> low|medium|high       # default priority hint for tasks with this tag
next tag clear-priority <tag>
next tag set-no-time-urgency <tag>                # disable age+due factors for tasks with this tag
next tag clear-no-time-urgency <tag>
next tag data set <tag> <key> <value>             # store arbitrary JSON value
next tag data get <tag> <key>
next tag data unset <tag> <key>
next tag data list <tag>
next tag show <tag>                               # display all metadata for a tag
```

Contexts (`@`), resources (`#`), and freeform tags are all stored identically under
`tags/` and committed to git. `next tag` is the unified command for tag metadata and tag
state alike; there are no kind-specific commands.

**Renaming**: `next tag rename` MUST cover every place a tag is recorded — the tag list
of every active task, the tag list of every archived task (warm `archive/` segments and
pruned cold segments), the tag's own metadata file, and the machine-local tag state that
references it. It is hierarchical:
renaming a tag moves every tag nested under it. The tag's kind (`@`, `#`, freeform) MUST
NOT change, since the leading character determines how the tag filters. Destination tags
that already exist are rejected unless `--merge` is given, in which case tasks carrying
both tags keep one copy and the destination's metadata wins. The committed half is one
commit; state is machine-local and is never committed. A cold segment can only be
rewritten by restoring it to the checkout, which a later archive pass prunes again.

**Tag filesystem encoding**: `@` and `#` prefixes are not safe on all platforms and
cause rendering issues in Forgejo. They are encoded on disk as `__context__` and
`__resource__` respectively: `@work` → `tags/__context__work.toml`,
`#printer` → `tags/__resource__printer.toml`. The encoding/decoding is transparent to
the user; all CLI and MCP interfaces continue to use `@` and `#` notation.

Tag name segments MUST NOT start with `__` (double underscore); this prefix is reserved
for internal filesystem encoding and is rejected by `validate_tag`.

### 8.6 Tree view

```
next tree [--all]
```

Shows all tasks in their parent-child hierarchy. Top-level tasks (no parent) appear as
roots; children are indented under their parent. `--all` includes done and cancelled
tasks; the default shows open and started tasks only.

### 8.7 User management

```
next user                       # show active user filter
next user set <name>...         # replace active user set
next user clear                 # clear user filter (show all users' tasks)
next user list                  # list all assignees found across all tasks
```

The user filter is NOT an access-control mechanism. All tasks are visible to all
operators. `active_users` is a personal workflow aid to focus the default view on the
tasks you are currently responsible for.

### 8.8 `next data`

Manage arbitrary key-value pairs on a task. Values may be any JSON type except null.

```
next data set <id-or-slug> <key> <value>     # set one key
next data unset <id-or-slug> <key>           # remove one key (errors if absent)
next data get <id-or-slug> <key>             # print value for one key
```

The value is parsed as JSON (number, boolean, array, object); anything that is not
valid JSON is stored as a plain string (e.g. `"42"` becomes the number `42`, `"true"`
becomes boolean `true`, `"hello"` stays a string). `null` is rejected.

**Key validation**: keys MUST be non-empty, at most 256 characters, and contain only
ASCII letters (`a-z`, `A-Z`), digits (`0-9`), hyphens (`-`), and underscores (`_`).
Dots, slashes, and spaces are not permitted.

### 8.9 Sync

```
next sync [--push-only] [--pull-only]
```

See §2.2. After a clean sync, any registered plugin whose periodic sync is due is run
(see §10.4).

### 8.10 Forecasting

```
next forecast [filters...] [--days N]
```

See §7.4.

### 8.11 Config

```
next config get [<key>]          # print one key, or all known keys when omitted
next config set <key> <value>    # set a key and save config.toml
```

Reads and writes the machine-local `config.toml` (see §9) without opening a repository.
Supported keys: `repository`, `list_limit` (`none` clears), `next_count`,
`forecast_horizon_days`, `sync.git_subprocess`, `sync.autopull`, `sync.autopush`,
`sync.staleness_secs`, `sync.pull_timeout_secs`. Unknown keys MUST be rejected.

### 8.12 Maintenance

```
next maintenance rebuild-cache   # drop & rebuild the local .next.db read cache
next maintenance archive         # run the archive pass now (see §2.3)
```

Repository maintenance operations, grouped under a `maintenance` namespace so the
everyday command list stays small (room for future integrity/cleanup operations).

- `rebuild-cache` MUST drop and rebuild the local SQLite read cache from the
  source-of-truth data (committed TOML files, git history, archive segments). It
  MUST NOT change any committed data and MUST NOT be treated as a mutation (no
  autopush, no push).
- The cache MUST self-heal on a format or build change: it stamps both the
  table-schema version and the writing binary's version, and when either differs
  from the running binary — an upgrade, a downgrade, or a cache left by a
  different or cache-unaware build — the next command that opens it MUST rebuild
  from the source-of-truth data before serving any query, so a version change can
  never surface stale, partial, or empty results. The rebuild is a one-time cost
  per change (the stamp is advanced afterwards) and touches no committed data.
- `archive` MUST run the archive pass on demand, bypassing the once-per-day
  throttle (see §2.3). It is a mutation (it commits) and follows the normal
  autopush rules. The archive pass is exposed *only* here — there is no
  top-level `next archive` command.

---

## 9. Storage and sync configuration

Tasks are stored as TOML files in a git repository. The repository root is determined by the
`--repo` flag, then `repository` in `$XDG_CONFIG_HOME/next/config.toml`, and otherwise
by walking up from the current working directory until a `.git` directory is found. There is
no pluggable-backend selection — the local git store is the only backend.

### 9.1 Sync configuration

Sync has two orthogonal capabilities, each a config key in the `[sync]` section
with a paired pair of CLI override flags:

- **autopull** (`sync.autopull`, default `true`; flags `--autopull` / `--no-autopull`) —
  the staleness pull before every command except `next sync`.
- **autopush** (`sync.autopush`, default `false`; flags `--autopush` / `--no-autopush`) —
  the push after a successful mutation.

The master flags `--autosync` / `--no-autosync` (and the alias `--offline`) toggle
both capabilities at once for a single invocation. Master and granular flags are
mutually exclusive (clap `conflicts_with`), so at most one determinant applies to each
capability. Resolution precedence per capability: granular flag → master flag → config.

```toml
[sync]
git_subprocess           = true    # default: false
autopull                 = true    # default: true
autopush                 = false   # default: false
staleness_secs           = 3600    # default: 3600 (1 hour)
pull_timeout_secs        = 10      # default: 10; stored only, not yet enforced
plugin_sync_default_secs = 86400   # default: 86400 (see §10.4)
```

Migration from the previous scheme: the old top-level `autosync` key is replaced by
`[sync] autopush`; `[sync] pull_before_query` is renamed to `[sync] autopull` (still
accepted as a serde alias); `[sync] offline` is removed (use `--offline` per invocation
or set `autopull`/`autopush` to `false`). The `--no-sync` flag is removed (use `--offline`).

When `git_subprocess = true`, `next sync` runs `git pull` and `git push` as
shell subprocesses instead of using the built-in libgit2 bindings. This is
useful when the system `git` handles authentication (SSH agents, credential
managers, 1Password, etc.) better than the embedded library. All other git
operations (commit, HEAD resolution) continue to use libgit2 regardless of
this setting.

### 9.2 Autopull and offline mode

When `autopull = true` (the default), every command except `next sync`
(and the repo-less `init`/`tutorial`/`config`) MUST first pull from the remote if
the machine-local `last_pull` timestamp is older than `staleness_secs`. The pull is
best-effort: a failure produces a warning and the command proceeds on possibly
stale data. A clean pull updates the cache and `last_pull`.

The `--no-autopull` flag, `--offline`, or `--no-autosync` MUST skip the autopull
for that invocation; `--autopull` / `--autosync` force it on. Symmetrically,
`--no-autopush` / `--offline` / `--no-autosync` suppress the post-mutation push and
`--autopush` / `--autosync` force it on.

`next sync` always pulls and pushes; it MUST fail with an actionable error when
invoked with any flag that disables either half (`--no-autopull`, `--no-autopush`,
`--no-autosync`, or `--offline`).

---

## 10. Integrations / plugins

Integrations (Forgejo, iCalendar/WebCal) are provided as **plugin binaries** — separate
processes, not built into the default core. The core provides the *export hook*: plugins
subscribe to individual tasks and are notified when those tasks change. A plugin MAY be a
fully external binary, or MAY be bundled in this crate behind a Cargo feature (off by
default, like `mcp`) and link the `next` library directly.

### 10.1 Registration

- Registration is performed via `next plugin …` CLI commands: `register <name> -- <argv>`
  (define/replace a plugin's export command, preserving subscriptions), `watch`/`unwatch
  <name> <task>`, `unregister <name>`, `set-sync <name> [--default-interval <secs>] --
  <argv>` (define/replace the periodic-sync command, upserting the plugin),
  `set-interval <name> <secs>|--clear` (user override of the sync interval),
  `enable`/`disable <name>` (toggle the periodic sync), and `list`.
- The registry MUST be machine-local — stored in the `[[plugin]]` section of the combined
  `state.toml` in the per-repo state directory (`$XDG_STATE_HOME/next/<hash>/`),
  never committed to git, guarded by the single machine-local state lock
  (`.state.toml.lock`) shared with the global and sync state, independent of the repo lock.
  Plugin commands are stored as argv (never shell-parsed).

### 10.2 Notification

- When a subscribed task is mutated (add/start/stop/done/cancel/edit/move/delete/data), the
  process performing the mutation (CLI or MCP server) MUST spawn each subscribed plugin's
  command **after the repository lock is released** (a plugin may call back into `next`).
- Spawning is **fire-and-forget** and best-effort; a failed or missing plugin MUST NOT fail
  the triggering command.
- The event is delivered on the child's stdin as JSON and in `NEXT_PLUGIN_EVENT`:
  `{ "event", "task_id", "repo", "timestamp" }`. `NEXT_REPO` and `NEXT_PLUGIN_ORIGIN`
  (the plugin name) are also set; cwd is the repo root.
- **Loop guard:** a plugin MUST NOT be notified of changes it caused itself. `next` sets
  `NEXT_PLUGIN_ORIGIN` when spawning a plugin; a `next` process running with that env set
  skips notifying the named plugin.
- A `delete` event is delivered, after which the task's subscriptions are pruned.

### 10.3 Forgejo plugin (`forgejo` feature)

The bundled `next-forgejo` binary (behind the off-by-default `forgejo` feature)
links the `next` library directly and maps Forgejo repositories to contexts.

- Config `~/.config/next-forgejo/config.toml`: `forgejo_url`, `forgejo_token`,
  optional `next_repo`, and `[[map]]` entries (`repo = "owner/repo"`, `context = "@ctx"`).
- `sync` MUST import each **open** issue with no linked task as a task tagged with the
  mapped context (title, issue url, body → description), link it via task data attributes
  `__forgejo-repo` / `__forgejo-issue` / `__forgejo-url` / `__forgejo-labels` (labels as a
  JSON array, NOT local tags), and subscribe the plugin to it. `--dry-run` MUST mutate
  nothing.
- Resolution is **close-only** and bidirectional: a closed issue marks its task done (on
  `sync`); a task resolved locally closes its issue (in real time via `hook`, and as a
  reconcile on `sync`). Reopening is out of scope for v1.
- `hook` reads `next` and mutates only Forgejo (never `next`), so it cannot loop.
- `sync` self-registers the export hook (idempotent) so per-task `watch` succeeds.

### 10.4 Periodic plugin sync

A plugin MAY declare a `sync_command` (via `next plugin set-sync`) — the import
direction, run on a schedule rather than per-event:

- After every successful `next sync`, each **enabled** plugin with a non-empty
  `sync_command` whose last successful sync is older than its resolved interval MUST be
  run to completion (cwd = repo root; `NEXT_REPO` and the `NEXT_PLUGIN_ORIGIN` loop
  guard set). This runs after the repository lock is released.
- The interval resolves with USER → PLUGIN → SYSTEM precedence: the user override
  (`set-interval`) wins, else the plugin's advertised default
  (`set-sync --default-interval`), else `plugin_sync_default_secs` from `config.toml`
  (default 86400).
- Unlike the fire-and-forget export hook, the run is synchronous and its exit status is
  checked: only a successful exit records `last_sync` (in the machine-local sync
  state), so a failure is retried on the next sync rather than suppressed for a whole
  interval.
- Failures MUST be isolated per plugin and MUST NOT fail the triggering sync.

---

## 11. Output

- Every list/show command MUST support `--json` emitting valid, stable JSON
- Plain-text output SHOULD use colour when stdout is a TTY; MUST fall back to plain
  text when stdout is not a TTY (pipe-safe)
- Date expressions in `--due` and `--start` MUST accept natural-language input
  ("tomorrow", "in two weeks", "next Monday", "2026-06-01") in addition to ISO 8601

---

## 12. MCP server (`next-mcp`)

The MCP server is an optional Cargo feature (`--features mcp`) that produces a second
binary. It implements the MCP Streamable HTTP transport (JSON-RPC 2.0 over HTTP POST).

### 12.1 Feature flag

- The `mcp` feature MUST be `off` by default
- All MCP runtime dependencies MUST be declared `optional = true` and activated only by the feature
- `cargo build` and `cargo test` without `--features mcp` MUST produce exactly the same
  artefacts as before the feature was added

### 12.2 Configuration

Configuration MUST be resolved as **env var > TOML config file > built-in default**.
The config-file path MUST be resolved as `--config <path>` > `NEXT_MCP_CONFIG` >
`NEXT_CONFIG` (deprecated alias) > `$XDG_CONFIG_HOME/next-mcp/config.toml` (the image
sets `XDG_CONFIG_HOME=/data/config`, giving `/data/config/next-mcp/config.toml`); an
absent or unparsable file falls back to defaults (with a warning when unparsable).
The file schema mirrors the variables: top-level `bearer_token`, `webhook_token`,
`repo_path`, `bind_addr`; `[git]` `url`/`user`/`token`/`author_name`/`author_email`/
`partial_clone`; `[sync]` `interval_secs`/`deferred_delay_secs`/`autopull` (accepts the
old name `pull_before_query` as an alias)/`staleness_secs`/`pull_timeout_secs`.

Each secret (`bearer_token`, `webhook_token`, `git.token`) MUST accept exactly one of
three forms — inline, `<name>_file` (a path, e.g. a podman secret at `/run/secrets/…`),
or `<name>_env` (the name of an env var holding the value) — with more than one form for
the same secret being a hard error. The matching legacy `NEXT_*` env var MUST still take
precedence and short-circuit resolution (so a stale `*_file` cannot break an env-based
deployment). This lets `config.toml` stay non-secret (references only).

| Variable | Required | Default |
|----------|----------|---------|
| `NEXT_BEARER_TOKEN` | ✓ (env or file) | — |
| `NEXT_MCP_CONFIG` | | `/data/config/next-mcp/config.toml` (`--config` wins; `NEXT_CONFIG` deprecated alias) |
| `NEXT_GIT_URL` | on first start | — |
| `NEXT_GIT_USER` / `NEXT_GIT_TOKEN` | | — |
| `NEXT_GIT_AUTHOR_NAME` / `NEXT_GIT_AUTHOR_EMAIL` | | `next-mcp` / `next-mcp@unknown` |
| `NEXT_REPO_PATH` | | `/data/tasks` |
| `NEXT_BIND_ADDR` | | `0.0.0.0:3000` |
| `NEXT_WEBHOOK_TOKEN` | | — |
| `NEXT_SYNC_INTERVAL` | | `86400` (s); `0` disables |
| `NEXT_DEFERRED_SYNC_DELAY_SECS` | | `30` (clamped to ≥ 1) |
| `NEXT_GIT_PARTIAL_CLONE` | | `1`; `0` forces the built-in full clone (no git binary needed) |
| `NEXT_AUTOPULL` | | `true`; `0`/`false`/`no` disables (old name `NEXT_PULL_BEFORE_QUERY` still read as a fallback) |
| `NEXT_STALENESS_SECS` | | `3600` |
| `NEXT_PULL_TIMEOUT_SECS` | | `10` (stored; not yet enforced) |

Credentials MAY alternatively be embedded in `NEXT_GIT_URL` as `https://user:token@host/repo.git`.

### 12.3 Git repository initialisation

On startup, `next-mcp` MUST:
1. If `NEXT_REPO_PATH/.git` exists: open the repository
2. Otherwise: clone `NEXT_GIT_URL` into `NEXT_REPO_PATH` using HTTPS credentials
3. Error and exit non-zero if neither condition is satisfied

The clone SHOULD be a partial clone (`--filter=blob:none`, via the `git`
binary) so only the current checkout's blobs transfer; any partial-clone
failure MUST fall back to the built-in full clone transparently, and
`NEXT_GIT_PARTIAL_CLONE=0` disables the attempt for deployments without a
git binary. Pruned cold-tier segment blobs absent from a partial clone are
fetched on demand when needed (§2.3).

The clone operation MUST be idempotent — a second start against the same volume MUST NOT re-clone.

After clone, `next-mcp` MUST set `user.name` and `user.email` in the local git config
if they are not already provided by global or system config, so that commits succeed
inside containers without a pre-configured git identity. The identity comes from
`NEXT_GIT_AUTHOR_NAME` / `NEXT_GIT_AUTHOR_EMAIL` (or `[git] author_name`/`author_email`
in the config file), defaulting to `next-mcp` / `next-mcp@unknown`.

### 12.4 Authentication

- All routes MUST require `Authorization: Bearer <NEXT_BEARER_TOKEN>`; return `401` otherwise
- The webhook route (`POST /webhook/sync`) MUST use a separate `NEXT_WEBHOOK_TOKEN`;
  neither token MUST be accepted on the other route
- Bearer token comparison MUST be constant-time and MUST NOT reveal the expected token's
  length through response-time differences (always process all bytes of the expected token)
- Request bodies MUST be limited to a small maximum size (≤ 64 KB) to resist memory-exhaustion attacks

### 12.5 MCP tools (14 total)

All existing CLI operations MUST be exposed as MCP tools: `list_tasks`, `get_task`,
`add_task`, `update_task`, `delete_task`, `sync`, `get_diff`, `force_sync`,
`get_state`, `set_tag_state`, `set_user_filter`, `manage_tag`,
`manage_task_data`, `get_forecast`. Mutation tools MUST accept an
`autosync: bool` parameter (default `true`):
- `autosync = true`: sync runs inline before the response is returned; sync errors are logged but MUST NOT fail the tool call
- `autosync = false`: a deferred sync is scheduled to fire after `NEXT_DEFERRED_SYNC_DELAY_SECS`; successive mutations MUST reset (not stack) the timer

The `sync` tool (optional `push_only`/`pull_only` booleans) MUST cancel any pending deferred timer and run sync immediately, surfacing errors to the caller. It MUST return an error immediately if a sync is already in progress rather than queuing.

**Conflict recovery tools:** `get_diff` MUST return the working-tree diff (git status
plus diff against HEAD) without mutating anything. `force_sync` MUST fetch from the
remote and hard-reset the working tree to `FETCH_HEAD`, discarding local changes and
merge conflicts; it does not push (the remote is the source of truth). Like `sync`, it
MUST fail fast when a sync is already in progress and cancels any pending deferred
timer.

**`list_tasks` pagination:** the tool accepts `page` (1-indexed, default 1),
`page_size` (default 50; `limit` is a legacy alias), `include_all`, and
`archived: true` to list the archive instead (most recently completed first; tag
filters and pagination apply, scoring does not). The result is
`{ items, page, page_size, total }`; `total > items.len()` signals truncation.

**Input validation (slug):** The `slug` field accepted by `add_task` and `update_task` MUST be validated using an allowlist: letters (`a-z`, `A-Z`), digits (`0-9`), hyphen (`-`), and underscore (`_`). No other characters are permitted. This prevents path traversal when the slug is used as the task's TOML filename (`tasks/<slug>.toml`).

**Input validation (tags):** Tags are validated by `domain::tag::validate_tag` using an allowlist per path segment: starts with an ASCII letter, then letters / digits / `-` / `_`. The `/` separator is allowed for hierarchical tags (e.g. `@home/kitchen`). The `..` component MUST be explicitly rejected. The `@` and `#` prefixes are permitted. Context tags (`@`) MUST be validated with `validate_context_tag` and resource tags (`#`) with `validate_resource_tag` to enforce the correct prefix.

**Input validation (data keys):** The `key` field in `manage_task_data` MUST be validated: non-empty, at most 256 characters, and contain only ASCII letters (`a-z`, `A-Z`), digits (`0-9`), hyphens (`-`), and underscores (`_`). Dots, slashes, and spaces MUST be rejected.

**`get_forecast` projection:** The `get_forecast` tool MUST have the same projection behaviour as `next forecast` (§7): in addition to concrete tasks due within the horizon, it MUST project active (open/started) schedule-type recurrence series forward to `today + horizon` using the shared core helper (`recurrence::project_series`), so the two implementations cannot drift. The horizon defaults to `DEFAULT_FORECAST_HORIZON_DAYS` (90) and is overridable via the `horizon_days` parameter. Each returned entry is `{ date, id, title, score, projected }`; projected (not-yet-spawned) occurrences MUST carry `projected: true` and concrete tasks `projected: false`. Completion-type recurrence and done/cancelled tasks MUST NOT be projected.

### 12.6 Sync mechanisms

Five independent sync triggers MUST coexist:

1. **Per-mutation autosync** — see §12.5
2. **Deferred timer** — fires `NEXT_DEFERRED_SYNC_DELAY_SECS` after the last `autosync=false` mutation; MUST be reset each time a new mutation arrives before the timer fires
3. **Periodic sync** — background task fires every `NEXT_SYNC_INTERVAL` seconds (0 = disabled)
4. **Webhook** (`POST /webhook/sync`) — protected by `NEXT_WEBHOOK_TOKEN`; fires sync immediately and cancels any pending deferred timer; returns `200 {"status": "synced" | "error", ...}`
5. **Autopull** — a best-effort staleness pull (same `pull_if_stale` core as
   the CLI, §9.2) before each task-touching tool call except `sync`, controlled by
   `NEXT_AUTOPULL` (old name `NEXT_PULL_BEFORE_QUERY` still read as a fallback) /
   `NEXT_STALENESS_SECS`; it never fails the tool call

At most one sync MUST run at a time. When an explicit sync (tool call or webhook) is already in progress, any concurrent explicit sync request MUST fail immediately with an error. Background syncs (deferred timer, periodic) MUST skip rather than queue when a sync is already running.

### 12.7 Container deployment

- A `Containerfile` MUST be provided for building the image
- A Podman Quadlet unit file (`quadlets/next-mcp.container`) MUST be provided
- The container MUST run as an unprivileged non-root user (UID 1000)
- Three named volumes MUST be used: `next-tasks` at `/data/tasks` (tasks repository), `next-state` at `/data/state` (XDG machine-local state via `XDG_STATE_HOME=/data/state`), and `next-config` at `/data/config` (TOML config file at `next-mcp/config.toml`, via `XDG_CONFIG_HOME=/data/config`)
- Secrets MUST be passed via podman `Secret=` mounts referenced by the config's `*_file` forms, an `EnvironmentFile` override, or an inline chmod-600 config file — never baked into the image
- Credentials embedded in `NEXT_GIT_URL` MUST be stripped before any log output; only the credential-free URL MAY be logged
