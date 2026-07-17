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

Machine-local state (active contexts, active users, resource availability) MUST be stored
at `$XDG_STATE_HOME/task-manager/<fnv1a-hash-of-repo-path>/state.toml`.  This path is
never inside the repository and MUST NOT be committed to git.

```toml
active_contexts   = ["@home"]       # active @ tags (empty = no filter)
excluded_contexts = ["@work"]       # always hide tasks with these contexts
active_users      = ["alice"]       # active user filter (empty = no filter)
[resources]
printer  = true
vacation = false
```

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

$XDG_STATE_HOME/task-manager/<repo-hash>/
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
day per machine (`last_archive` in the machine-local state); `next archive`
runs it on demand without the throttle. Archive/prune failures MUST NOT fail
the sync.

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

Tags are strings in the `tags` array of a task. Prefix conventions:

| Prefix | Kind | Example |
|--------|------|---------|
| `@` | Context | `@home`, `@work` |
| `#` | Resource | `#printer`, `#vacation` |
| *(none)* | Freeform | `python`, `reading`, `project` |

### 3.1 Context filtering

When one or more `@` contexts are active:
- Tasks with **no** `@` tags MUST always be included
- Tasks with at least one `@` tag MUST be included only if they share at least one `@` tag with the active set
- Tasks whose `@` tags are all outside the active set MUST be excluded

When **no** context is active, all tasks MUST be shown regardless of their `@` tags.

A query-time `context:@name` filter MUST override the global active-context set for that
single invocation.

### 3.1.1 Excluded contexts

`state.toml` MAY contain an `excluded_contexts` list. Tasks whose `@context` tags match
any excluded context MUST be hidden, even when they would otherwise pass the active-context
filter. Context-neutral tasks (no `@` tags) are never excluded.

Exclusion matching is one-directional: excluding `@home` hides tasks tagged `@home` or any
descendant (e.g. `@home/kitchen`), but excluding `@home/kitchen` does NOT hide tasks tagged
only with `@home`.

CLI: `next context exclude <@tag>...` / `next context clear-excluded`
MCP: `set_context` accepts an optional `excluded_contexts` array.

### 3.2 Resource filtering

A resource `#<name>` is available when `resources.<name> = true` in `state.toml` (absent
keys default to **available**).

Tasks that carry a `#<name>` tag where `<name>` is **unavailable** MUST be excluded from
the default list and from score computation.

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

The score MUST be the sum of the following weighted factors:

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

Tasks excluded from the default view (blocked, resource-unavailable, future `start`,
parent awaiting subtasks) MUST NOT receive a score and MUST NOT appear in `next list` /
`next next` output unless `--all` is passed.

---

## 5. Filtering and queries

All list commands MUST accept the following filter tokens, freely combinable:

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

When `active_contexts` is non-empty and the new task carries no `@context` tags,
the active contexts MUST be automatically appended to the task's `tags` array.
If the user supplies any `@` tag, auto-apply is skipped.

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
tasks (most recently completed first; tag filters and pagination apply, scoring and the
implicit gate do not). `--archived` conflicts with `--all`, `--closed`, and `--future`.

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

### 8.4 Context and resource management

```
next context                             # show active and excluded contexts (with descriptions)
next context set <@tag>...               # replace active context set
next context clear                       # clear all active contexts
next context exclude <@tag>...           # replace excluded context set (see §3.1.1)
next context clear-excluded              # clear all excluded contexts

next resource                            # list resources and availability (with descriptions)
next resource set <#tag> on|off          # toggle a resource
```

### 8.5 Tag metadata

```
next tag                                          # list all tags grouped by kind
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
`tags/` and committed to git. `next tag` is the unified command for all tag metadata —
there are no separate describe/clear commands on `next context` or `next resource`.
Descriptions appear in `next context`, `next resource`, and `next tag` output.

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
Supported keys: `autosync`, `repository`, `list_limit` (`none` clears), `next_count`,
`forecast_horizon_days`, `sync.git_subprocess`, `sync.pull_before_query`,
`sync.staleness_secs`, `sync.pull_timeout_secs`. Unknown keys MUST be rejected.

---

## 9. Storage and sync configuration

Tasks are stored as TOML files in a git repository. The repository root is determined by the
`--repo` flag, then `repository` in `$XDG_CONFIG_HOME/task-manager/config.toml`, and otherwise
by walking up from the current working directory until a `.git` directory is found. There is
no pluggable-backend selection — the local git store is the only backend.

### 9.1 Sync configuration

The `[sync]` section of `config.toml` controls how `next sync` (and autosync) performs push/pull:

```toml
[sync]
git_subprocess           = true    # default: false
pull_before_query        = true    # default: true
staleness_secs           = 3600    # default: 3600 (1 hour)
pull_timeout_secs        = 10      # default: 10; stored only, not yet enforced
offline                  = false   # default: false
plugin_sync_default_secs = 86400   # default: 86400 (see §10.4)
```

When `git_subprocess = true`, `next sync` runs `git pull` and `git push` as
shell subprocesses instead of using the built-in libgit2 bindings. This is
useful when the system `git` handles authentication (SSH agents, credential
managers, 1Password, etc.) better than the embedded library. All other git
operations (commit, HEAD resolution) continue to use libgit2 regardless of
this setting.

### 9.2 Pull-before-query and offline mode

When `pull_before_query = true` (the default), every command except `next sync`
(and the repo-less `init`/`tutorial`/`config`) MUST first pull from the remote if
the machine-local `last_pull` timestamp is older than `staleness_secs`. The pull is
best-effort: a failure produces a warning and the command proceeds on possibly
stale data. A clean pull updates the cache and `last_pull`.

The `--offline` global flag (alias `--no-sync`), or `offline = true` in `[sync]`,
MUST skip both the pull-before-query and the autosync push for that invocation.
`--offline`/`--no-sync` conflict with `--autosync`.

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
  `state.toml` in the per-repo state directory (`$XDG_STATE_HOME/task-manager/<hash>/`),
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
The config file is read from `NEXT_CONFIG` if set, else `/data/config/config.toml`;
an absent or unparsable file falls back to defaults (with a warning when unparsable).
The file schema mirrors the variables: top-level `bearer_token`, `webhook_token`,
`repo_path`, `bind_addr`; `[git]` `url`/`user`/`token`/`author_name`/`author_email`/
`partial_clone`; `[sync]` `interval_secs`/`deferred_delay_secs`/`pull_before_query`/
`staleness_secs`/`pull_timeout_secs`.

| Variable | Required | Default |
|----------|----------|---------|
| `NEXT_BEARER_TOKEN` | ✓ (env or file) | — |
| `NEXT_CONFIG` | | `/data/config/config.toml` |
| `NEXT_GIT_URL` | on first start | — |
| `NEXT_GIT_USER` / `NEXT_GIT_TOKEN` | | — |
| `NEXT_GIT_AUTHOR_NAME` / `NEXT_GIT_AUTHOR_EMAIL` | | `next-mcp` / `next-mcp@unknown` |
| `NEXT_REPO_PATH` | | `/data/tasks` |
| `NEXT_BIND_ADDR` | | `0.0.0.0:3000` |
| `NEXT_WEBHOOK_TOKEN` | | — |
| `NEXT_SYNC_INTERVAL` | | `86400` (s); `0` disables |
| `NEXT_DEFERRED_SYNC_DELAY_SECS` | | `30` (clamped to ≥ 1) |
| `NEXT_GIT_PARTIAL_CLONE` | | `1`; `0` forces the built-in full clone (no git binary needed) |
| `NEXT_PULL_BEFORE_QUERY` | | `true`; `0`/`false`/`no` disables |
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

### 12.5 MCP tools (15 total)

All existing CLI operations MUST be exposed as MCP tools: `list_tasks`, `get_task`,
`add_task`, `update_task`, `delete_task`, `sync`, `get_diff`, `force_sync`,
`get_state`, `set_context`, `set_resource`, `set_user_filter`, `manage_tag`,
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
5. **Pull-before-query** — a best-effort staleness pull (same `pull_if_stale` core as
   the CLI, §9.2) before each task-touching tool call except `sync`, controlled by
   `NEXT_PULL_BEFORE_QUERY` / `NEXT_STALENESS_SECS`; it never fails the tool call

At most one sync MUST run at a time. When an explicit sync (tool call or webhook) is already in progress, any concurrent explicit sync request MUST fail immediately with an error. Background syncs (deferred timer, periodic) MUST skip rather than queue when a sync is already running.

### 12.7 Container deployment

- A `Containerfile` MUST be provided for building the image
- A Podman Quadlet unit file (`quadlets/next-mcp.container`) MUST be provided
- The container MUST run as an unprivileged non-root user (UID 1000)
- Three named volumes MUST be used: `next-tasks` at `/data/tasks` (tasks repository), `next-state` at `/data/state` (XDG machine-local state via `XDG_STATE_HOME=/data/state`), and `next-config` at `/data/config` (optional TOML config file)
- Secrets MUST be passed via an `EnvironmentFile` or the config file (chmod 600), not baked into the image
- Credentials embedded in `NEXT_GIT_URL` MUST be stripped before any log output; only the credential-free URL MAY be logged
