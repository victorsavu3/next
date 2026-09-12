# `next` CLI Reference

A task manager with automatic urgency scoring. The binary is called `next`.

A full-screen terminal front-end, `next-tui`, is also available — see [`TUI.md`](TUI.md).

---

## Command Summary

| Command | Short description |
|---------|------------------|
| `next init` | Initialise a task repository in the current directory |
| `next add` | Add a new task |
| `next list` | List tasks sorted by urgency score |
| `next next` | Show the top N highest-scored tasks |
| `next show` | Show full details of a single task |
| `next tree` | Show all tasks in a parent-child tree |
| `next start` | Mark a task as started (in-progress); logs a time entry |
| `next stop` | Stop a started task (back to open); logs a time entry |
| `next done` | Mark a task complete (triggers recurrence if applicable) |
| `next cancel` | Mark a task cancelled |
| `next edit` | Edit fields on an existing task |
| `next delete` | Permanently delete a task (with confirmation) |
| `next move` | Change the parent of a task |
| `next open` | Open the task's URL in the default browser |
| `next data set` | Set a key in the task's data map |
| `next data unset` | Remove a key from the task's data map |
| `next data get` | Print the value of one key from the task's data map |
| `next tag` | List all tags with descriptions, grouped by kind |
| `next tag rename` | Rename a tag (and its descendants) across the repository |
| `next tag describe` | Set a description for any tag |
| `next tag clear-description` | Remove a tag description |
| `next tag require` | Work on these tags: only their tasks are listed |
| `next tag exclude` | Hide tasks carrying these tags |
| `next tag accept` | Pin tags to neither required nor excluded, ignoring a parent tag's state |
| `next tag clear-state` | Drop the stored state for tags (all of them when none given) |
| `next tag show` / `set-url` / `clear-url` / `set-priority` / `clear-priority` / `data ...` | More tag metadata; see [Other `next tag` subcommands](#other-next-tag-subcommands) |
| `next forecast` | Show upcoming recurrence dates |
| `next sync` | Pull from git remote, reconcile cache, auto-archive if due, push |
| `next maintenance archive` | Move old closed tasks into archive segments now |
| `next maintenance rebuild-cache` | Drop and rebuild the local `.next.db` read cache |
| `next user` | Show active user filter |
| `next user set` | Set the global active user filter |
| `next user clear` | Clear the user filter |
| `next user list` | List all assignees across all tasks |
| `next tag set-no-time-urgency` | Disable age+due urgency factors for tasks with a tag |
| `next tag clear-no-time-urgency` | Re-enable time-based urgency for tasks with a tag |
| `next plugin` | Manage export plugins and their periodic syncs |
| `next config` | Read or write a value in the machine-local `config.toml` |
| `next tutorial` | Print the embedded tutorial |

Running `next` with no subcommand is the same as `next list`.

---

## Global flags

Available on every subcommand:

| Flag | Description |
|------|-------------|
| `--config <path>` | Config file to use (default: `$XDG_CONFIG_HOME/next/config.toml`). |
| `--repo <path>` | Task repository root. Overrides `repository` in the config and the upward `.git` search. |
| `--autopull` / `--no-autopull` | Force the pre-command staleness pull on or off for this invocation. Overrides `sync.autopull` in config. |
| `--autopush` / `--no-autopush` | Force the post-mutation push on or off for this invocation. Overrides `sync.autopush` in config. |
| `--autosync` | Master switch: enable **both** autopull and autopush for this invocation. |
| `--no-autosync` | Master switch: disable **both** autopull and autopush for this invocation. |
| `--offline` | Alias for `--no-autosync`: disable both — no network I/O. |
| `--no-progress` | Never draw progress bars, even on a terminal. |
| `--quiet`, `-q` | Suppress progress bars **and** the informational notes on stderr (the auto-pull note, the `Synced with remote.` confirmation). Errors and results are unaffected. |
| `--log-level <LEVEL>` | Diagnostic log verbosity for this invocation: `error`, `warn`, `info`, `debug`, or `trace`. Sets the default level, overriding `RUST_LOG`'s default of `error`; any `RUST_LOG` per-target directives still apply on top. |

Sync has two orthogonal capabilities: **autopull** (the pre-command staleness pull) and
**autopush** (the push after a successful mutation). Each has a config key
(`sync.autopull`, default true; `sync.autopush`, default false) and a paired override flag
pair. The master flags (`--autosync` / `--no-autosync` / `--offline`) toggle both at once
and are mutually exclusive with each other and with the granular flags.

Unless disabled (`sync.autopull = false` in config, or `--no-autopull` / `--offline`), every
command except `next sync` first runs a best-effort **autopull**: if the last
pull is older than `sync.staleness_secs` (default 1 hour) it pulls from the remote, so
results reflect other machines' pushes. A failed pull prints a warning and the command
proceeds on the local data.

`next sync` always pulls **and** pushes; it refuses to run (with an error) if invoked with
any flag that would disable either half (`--no-autopull`, `--no-autopush`, `--no-autosync`,
or `--offline`).

---

## Progress reporting

Operations that can take a while — the cache rebuild on a fresh clone, the archive
pass, a fetch or push, a tag rename — draw a spinner or a bar while they run:

```
Reading git history ⠹ (2s)
Indexing tasks [========>---------------] 4210/12480 tasks/plant-the-beds.toml (3s, eta 5s)
Pulling [==============>---------] 812/1300 receiving objects (1s, eta 1s)
```

Three things to know about it:

- **It is drawn on stderr and cleared when the phase ends.** stdout carries only
  results, so `next list --json | jq` and `next show x > file` are unaffected,
  and nothing is left on screen afterwards.
- **Nothing appears for the first 100 ms.** Fast operations — which is most of
  them — never flash a bar. The wait for a stalled remote does appear, because
  the delay is a timer rather than a "show it once something happens".
- **It stays out of the way of scripts.** Progress is drawn only when stderr is
  a terminal, `TERM` is not `dumb`, the command is not printing JSON
  (`--json` / `--format json`), and neither `--quiet` nor `--no-progress` was
  given.

`--no-progress` turns off the bars alone. `--quiet` (`-q`) turns off the bars *and*
the informational notes — `note: pulled latest changes…`, `warning: auto-pull
failed…`, `Synced with remote.` — while leaving errors and results alone.

### `NEXT_FORCE_PROGRESS`

```sh
NEXT_FORCE_PROGRESS=1 next maintenance rebuild-cache --json 2>rebuild.log
```

Setting `NEXT_FORCE_PROGRESS=1` renders progress even when the automatic rules say
not to — a redirected stderr, a JSON command, `TERM=dumb` — and skips the 100 ms
delay so that a forced run always paints something. It does **not** override
`--quiet` or `--no-progress`: an explicit "no" from the command line wins over an
environment variable.

It is there for two cases: deliberately capturing progress into a log while stderr
is redirected, and testing the rendering path without a pseudo-terminal.

---

## Command Reference

### `next init`

Initialise a new task repository in the current directory. Safe to run on an existing repository.

**Usage**

```
next init
```

What it does:

1. Runs `git init` (skipped if `.git` already exists)
2. Creates `tasks/` directory
3. Writes `config/scoring.toml` with the default scoring weights (skipped if present)
4. Appends the generated-file entries (`.next.db`, `.next.db-wal`, `.next.db-shm`, `.next.lock`, `state.toml`) to `.gitignore` (or creates `.gitignore`; never duplicates entries)
5. Creates an initial git commit when git user config is available

---

### `next add`

Add a new task.

**Usage**

```
next add <title> [options]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<title>` | string | — | Task title (required) |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--due <expr>` | date expression | none | Deadline. Accepts natural language ("in two weeks", "next Monday", "tomorrow") or ISO 8601 ("2026-06-01"). |
| `--start <expr>` | date expression | none | Task is hidden until this date. Same syntax as `--due`. Disables age scoring for this task. |
| `--priority <level>` | `low\|medium\|high` | `medium` | Urgency priority. |
| `--slug <slug>` | string | none | Stable short identifier used as the filename and for referencing. Must be unique. |
| `--tag <tag>` | string | none | Add a tag. Repeatable. Use `@` prefix for context tags, `#` prefix for resource tags. |
| `--parent <id>` | task ID | none | Makes this task a subtask of the given task. The parent is blocked until all children complete. |
| `--blocked-by <id>` | task ID | none | Declares an explicit blocker. Repeatable. |
| `--description <text>` | string | none | Multi-line description providing context beyond the title. |
| `--url <url>` | string | none | URL associated with this task (must be http or https). |
| `--notes <text>` | string | none | Multi-line free-text notes. |
| `--recur-schedule <rule>` | RRULE string | none | Schedule-based recurrence. The rule is an RFC 5545 RRULE string (without the `RRULE:` prefix). See [Recurrence](#recurrence) below. |
| `--recur-completion <days>` | positive integer | none | Completion-based recurrence. The next instance is created `<days>` after the task is marked done. Must be at least 1. |
| `--recur-snap <snap>` | snap value | none | Move the computed next date to a qualifying date. See [Snap values](#snap-values) below. Applies to both schedule and completion modes. |
| `--recur-snap-leeway <spec>` | `N`, `BACK,FORWARD` or `BACK,*` | none | How far the snap may move the date, in whole days (0–365); `*` leaves the forward direction unbounded. Without it a snap only ever moves dates **later**, however far. Requires `--recur-snap`. See [Snap leeway](#snap-leeway---recur-snap-leeway) below. |
| `--long-term` | flag | false | Disables the age factor from scoring. Suitable for background or long-running tasks. |
| `--adjust <value>` | float | 0.0 | Manual score adjustment added directly to the computed urgency score. Positive boosts, negative penalises. |
| `--assignee <name>` | string | none | Assign the task to a user. Used by the user filter. |
| `--json` | flag | false | Emit the created task as JSON on stdout. |

**Examples**

```sh
# Minimal capture
next add "Call dentist"

# Task with deadline and context tag
next add "Submit tax forms" --due "April 15" --tag @home --priority high

# Project task (a plain task; children attach to it via --parent)
next add "Launch blog" --slug launch-blog --priority high

# Subtask under an existing project
next add "Write first post" --parent launch-blog

# Recurring: water plants 7 days after last watering, snapped to Saturday
next add "Water plants" --slug water-plants --recur-completion 7 --recur-snap saturday --recur-snap-leeway 2 --tag @home

# Recurring: rent on the 1st, and a two-day slip must not push it to next month
next add "Pay rent" --slug rent --due 2026-06-01 \
  --recur-completion 30 --recur-snap dom:1 --recur-snap-leeway 3

# Recurring: daily standup every weekday
next add "Daily standup" --slug standup --recur-schedule "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"

# Recurring: monthly task starting the 1st, due the 3rd
next add "Monthly review" --recur-schedule "FREQ=MONTHLY;BYMONTHDAY=1" --start 2026-06-01 --due 2026-06-03

# Task with description and URL
next add "Review RFC-42" --url "https://example.com/rfc-42" --description "Pay attention to section 3"
```

---

### `next list`

List all tasks that pass the active filters, sorted by urgency score descending. Overdue and due-today tasks are highlighted at the top.

**Usage**

```
next list [options] [filters...]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--future` | flag | false | Include tasks with a future `start` date and planned recurrence instances. |
| `--all` | flag | false | Disable all implicit filtering: contexts, resources, blocked tasks, and future `start` dates. |
| `--closed` | flag | false | Show only closed tasks (done or cancelled). Can be combined with `--all`. Closed tasks all score 0, so the listing keeps the store order: most recently completed first, the same order as `--archived`. |
| `--archived` | flag | false | List archived tasks instead, most recently completed first. Tag filters and pagination apply; scoring and the implicit gate do not. Conflicts with `--all`, `--closed`, and `--future`. |
| `--all-users` | flag | false | Bypass the user filter; show tasks for all assignees. |
| `-n` / `--limit` | integer | — | Show at most N tasks. Shorthand for `--page-size`; overrides `list_limit` in config. |
| `--page-size` | integer | 50 | Tasks per page. |
| `--page` | integer | 1 | 1-indexed page of results to show. |
| `--json` | flag | false | Emit the result as JSON: `{ "items": [...], "page": N, "page_size": N, "total": N }`. The older spelling of `--format json`; the two cannot be combined. |
| `--format <fmt>` | `table` \| `json` | `table` | Output format. Table and json are the whole list, deliberately; `--format` exists so `--json` is not the only vocabulary, not because a third format is coming. |
| `--count` | flag | false | Print only how many tasks match, and nothing else. Counted before pagination, so the answer is the total across every page rather than the size of the one you happened to ask for. |
| `--explain` | flag | false | Print what the filter parsed to and where it will run, instead of listing anything. See [Explaining a query](#explaining-a-query). |
| `--fields <list>` | comma-separated names | everything | Return only these fields, e.g. `--fields id,title,due`. JSON output only — without `--json` it is a usage error, not a silent no-op. See [Choosing which fields come back](#choosing-which-fields-come-back). |

The trailing arguments are the filter (see [Filter Syntax](#filter-syntax)); every
flag must come **before** them. `next list --all +@work` is right, and
`next list +@work --all` is refused with a message saying so. The trailing part is
taken verbatim so that `-bug` and `-#printer` keep working as exclusions, which
leaves nothing to distinguish a trailing `--all` from a term — so the command
refuses rather than guesses. Exclusions are unaffected: they were always trailing
and still are.

A default cap can be set in config as `list_limit = N`. The precedence is
`--page-size`, then `-n` / `--limit`, then `list_limit`, then 50, and `--archived`
follows the same one — an archive listing is still a listing, and a cap set once
should not stop applying because the tasks moved tier. When the result is a window
on a larger set, text output ends with an indication such as
`page 2 of 14 · 13402 matching · --page 3 for more`.

A query that contradicts the implicit gate — `status:done`, `is:closed`,
`--closed status:open` — matches nothing, because the gate removes those tasks
before the expression is ever evaluated. The listing prints a hint naming `--all`
when it sees one; which tasks match is unchanged. See
[A status predicate can contradict the gate](#a-status-predicate-can-contradict-the-gate).

**Examples**

```sh
# Default view (respects the tag state)
next list

# All Python-tagged tasks
next list +python

# Tasks available at home, including those not yet started
next list --future context:@home

# Everything, bypassing all implicit filters
next list --all

# Archived work-tagged tasks, second page
next list --archived --page 2 +@work

# How many tasks match, without printing any of them
next list --count +@work

# Why did that return nothing?
next list --explain status:done

# A lean payload for a script: three fields per task, no notes
next list --json --fields id,title,due
```

---

### `next maintenance`

Repository maintenance operations. These are grouped under `maintenance` to keep
them out of the everyday command list; the namespace has room for future
operations (e.g. integrity checks, lock cleanup).

```
next maintenance rebuild-cache [--json]
next maintenance archive [--json]
```

#### `next maintenance rebuild-cache`

Drop and rebuild the local SQLite read cache (`.next.db`) from the committed
TOML files, git history, and archive segments. The cache is normally maintained
automatically; use this only to recover from a corrupt or stale cache. It changes
no committed data and makes no network calls, so it is not a mutation and never
triggers a sync. Prints the number of active tasks in the rebuilt cache
(`{ "rebuilt": true, "active_tasks": N }` with `--json`).

#### `next maintenance archive`

Move old closed tasks into archive segments now, bypassing the daily automatic
throttle. Tasks whose completion (or, for cancelled tasks, last update) is older
than `archive_after_days` — 180 by default, configurable in the committed
`config/archive.toml` — leave the `tasks/` directory for month-keyed segment
files under `archive/`. When `prune_after_days` is set, segments past that
threshold are additionally pruned from the checkout (recoverable via git).

Archived tasks stay visible in `next list --archived` and resolve by id or slug
everywhere. Editing one brings it back automatically; deleting one is an error
until it is edited back. The same pass runs automatically during sync at most
once per day (disable with `auto = false` in `config/archive.toml`).

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit `{ "archived": N, "segments": [...], "pruned": [...] }`. |

---

### `next next`

Show the top N tasks by urgency score. This is the primary "what should I do now?" command.

**Usage**

```
next next [options] [N] [filters...]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `[N]` | positive integer | 10 | Number of tasks to show. Defaults to `next_count` in config. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--future` | flag | false | Include tasks with a future `start` date. |
| `--all` | flag | false | Disable all implicit filtering: contexts, resources, blocked tasks, and future `start` dates. |
| `--all-users` | flag | false | Bypass the user filter; show tasks for all assignees. |
| `--json` | flag | false | Emit the tasks as JSON. Alias for `--format json`. |
| `--format <fmt>` | `table` \| `json` | `table` | Output format. |
| `--fields <list>` | comma-separated names | everything | Project the JSON output, exactly as on `next list`. JSON output only. |

The same filter expression as `next list`, and the same rule that every flag
precedes **the filter**. `[N]` is not the boundary — `next next --json 5 +@work`
and `next next 5 --json +@work` both work, because the trailing filter only
starts collecting at the first token that is not the count. It is
`next next 5 +@work --json` that is refused: from `+@work` onward the line is
taken verbatim, so a flag there would be read as a filter term.

There is no `--count` and no pagination: `[N]` already caps the
output, and a command whose whole job is the top of the list has nothing useful
to say about how many tasks matched in total — `next list --count` is the place
to ask that.

**Examples**

```sh
next next
next next 5 context:@work
next next --json
next next --json --fields id,title,score
```

---

### `next show`

Display the full details of a single task, including its description, URL, data, notes, all tags, subtask list, blockers, recurrence configuration, and urgency score breakdown (due, priority, age, tag, and other factors shown inline). A closed (done or cancelled) task scores 0 and shows no breakdown.

**Usage**

```
next show <id> [--json] [--fields <list>]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID, slug, or any unambiguous prefix (minimum 4 hex characters). |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit full task details as JSON: `{ "task": …, "score": …, "score_breakdown": …, "children": [ … ] }`. |
| `--fields <list>` | comma-separated names | everything | Return only these fields, e.g. `--fields id,title,status`. Applies to the task and its children alike — a caller asking for `id,title` wants that shape throughout, not one trimmed task beside a set of full ones. JSON output only. |

There is no `--format` here: `next show` renders one task, and its text form is a
labelled block rather than a table, so there is no second layout to pick between.

**The recurrence block**

A recurring task gets three lines: the rule, the snap with its leeway, and the date the
next instance would get if you completed the task today. The last is computed by the same
function `next done` runs, so it cannot disagree with what completing actually produces.

```
Recur:    30d after completion
Snap:     day 1 of month, leeway 3d back / 3d forward
Next:     2026-07-01  (if completed today)
```

A schedule rule shows its anchor too, since the anchor decides which dates the rule can
produce at all and a schedule is not reproducible without it:

```
Recur:    schedule (FREQ=WEEKLY;BYDAY=MO), anchor 2026-05-04
Snap:     none
```

With a snap but no leeway the line reads `day 1 of month, no leeway (always moves later)`.
The parenthetical is deliberate: the default is a policy, not the absence of one, and it
is the policy that lengthens a cycle whenever the task is completed late. A rule whose
`UNTIL` or `COUNT` is spent shows `Next:     none  (the rule is exhausted)`.

Under `--fields`, the computed `score` and `score_breakdown` come back only when
you name them — they sit *beside* the task rather than inside it, and a projection
that kept them regardless would contradict itself. `next list` has always behaved
this way; `next show` and the MCP `get_task` now match it. Without `--fields` the
response is unchanged and carries everything.

---

### `next tree`

Show all tasks in a parent-child tree. Root tasks (no parent) are listed at the top;
child tasks are indented under their parent.

**Usage**

```
next tree [options] [filters...]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--all` | flag | false | Include done and cancelled tasks. Default shows open tasks only. |
| `--closed` | flag | false | Show only done and cancelled tasks, still respecting the active context and the filter. |
| `--json` | flag | false | Emit a flat task list as JSON, each entry carrying `parent_id` — JSON has no indentation to carry the shape, so the edges are named instead. Alias for `--format json`. |
| `--format <fmt>` | `table` \| `json` | `table` | Output format. |
| `--count` | flag | false | Print only how many tasks matched. Honoured with `--json` as well as without. |
| `--fields <list>` | comma-separated names | everything | Project the JSON output, exactly as on `next list`. JSON output only. |

`next tree` takes the same filter expression as `next list`, with the same rule
that every flag precedes it. A task is shown when it matches — and so are its
ancestors, because a tree with the branches removed would leave matching subtasks
floating with no visible parent.

Those ancestors are scaffolding rather than results, so `--count` does not count
them: it reports how many tasks the filter matched, not the number of lines the
tree happens to draw.

That is **not** always the number `next list --count` gives for the same query.
The two commands gate differently, and deliberately: `next list` applies the
implicit gate, which hides blocked tasks and parents with open subtasks, while
`next tree` cannot — a tree that dropped them would have no shape left to show.
So `next tree --count is:blocked` reports the blocked tasks and
`next list --count is:blocked` reports none. Where the gate does not bite, the
two agree; that is a coincidence of the query, not a guarantee.

**Examples**

```sh
# Tree of open tasks
next tree

# Full tree including completed work
next tree --all

# Only the tasks mentioning "printer", with their parents kept for shape
next tree printer

# How many tasks match, without drawing anything
next tree --count +@work
```

---

### `next start`

Mark a task as started (in-progress). Started tasks stay visible in `next list` /
`next next`, receive a flat started bonus in scoring, and count as active for blocking
and parent-child checks. Appends `{event: "start", at: <timestamp>}` to the task's
`data["time_log"]` array.

**Usage**

```
next start <id> [--json]
```

---

### `next stop`

Stop a started task (status returns to open). Appends `{event: "stop", at: <timestamp>}`
to `data["time_log"]`. Entries accumulate across start/stop cycles for time-tracking
analysis.

**Usage**

```
next stop <id> [--json]
```

---

### `next done`

Mark a task as complete. If the task has a recurrence rule, the next instance is created automatically.

**Usage**

```
next done <id>
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--completed-at <date>` | date expression | today | Date the task was completed. Recorded on the task and used for recurrence scheduling. ISO 8601 or natural language. |
| `--json` | flag | false | Emit the completed task as JSON. A spawned recurrence instance is **not** included — one object is printed. Use `next list` or `next forecast` afterwards to see the new instance. |

**Notes**

The completion date is stored on the task in its `completed_at` field (shown as `Completed:` by `next show`) and drives completion-based recurrence scheduling. It is independent of the `updated_at` timestamp, so `--completed-at` lets you backdate a task you forgot to mark done on the day it was actually completed.

---

### `next cancel`

Mark a task as cancelled. Cancelled tasks are removed from the default view and are no longer considered blockers.

**Usage**

```
next cancel <id>
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit the updated task as JSON. |

---

### `next edit`

Modify one or more fields on an existing task. Only the flags you supply are changed; all other fields are left untouched.

**Usage**

```
next edit <id> [options]
```

**Options**

Same flags as `next add`, plus:

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--title <text>` | string | — | Replace the task title. |
| `--clear-due` | flag | false | Remove the due date. |
| `--clear-start` | flag | false | Remove the start date. |
| `--clear-parent` | flag | false | Remove the parent relationship (promote to top-level task). |
| `--remove-tag <tag>` | string | — | Remove a specific tag. Repeatable. |
| `--clear-blocked-by` | flag | false | Remove all explicit blockers. |
| `--clear-assignee` | flag | false | Remove the assignee. |
| `--clear-description` | flag | false | Remove the description. |
| `--clear-url` | flag | false | Remove the URL. |
| `--clear-recurrence` | flag | false | Remove the recurrence rule and `recurrence_id`. |
| `--clear-recur-snap` | flag | false | Remove the snap **and its leeway** — a leeway has nothing to mean without a boundary. Keeps the recurrence rule. Rejected together with `--recur-snap`. |
| `--clear-recur-snap-leeway` | flag | false | Remove only the leeway, restoring the default: never pull a date earlier, always push it later. Rejected together with `--recur-snap-leeway`. |
| `--json` | flag | false | Emit the updated task as JSON. |

`--recur-schedule`, `--recur-completion`, `--recur-snap` and `--recur-snap-leeway` work the same as in `next add`. When editing a schedule rule, the original `anchor` date is preserved so interval alignment stays correct. `--recur-snap` and `--recur-snap-leeway` can also be used standalone to change the snap or its tolerance on an existing recurring task without re-specifying the full rule.

Changing the rule keeps the snap and the leeway: `next edit water-plants --recur-completion 31` leaves an existing `dom:1` snap and its leeway in place. Use `--recur-snap` / `--recur-snap-leeway` to replace them, `--clear-recur-snap-leeway` to drop the tolerance alone, or `--clear-recur-snap` to drop both — any of these may be combined with a rule change or sent on its own.

Trailing `+tag` and `-tag` tokens may also be used to add or remove tags:

```sh
next edit water-plants +@garden -urgent
```

**Examples**

```sh
next edit a1b2 --due "next Friday"
next edit a1b2 --priority high --tag @work
next edit a1b2 --remove-tag @home --clear-due
next edit a1b2 --url "https://example.com/ticket-42" --description "See comments in ticket"
next edit standup --recur-snap monday        # change snap without re-specifying the rule
next edit rent --recur-snap-leeway 5,0       # pull back up to 5 days, never push past the 1st
next edit rent --clear-recur-snap-leeway     # back to the forward-only default
next edit standup --clear-recur-snap         # drop the snap and its leeway, keep the rule
next edit old-task --clear-recurrence        # remove the recurrence rule entirely
```

---

### `next delete`

Permanently remove a task from the repository. Prompts for confirmation unless `--yes` is passed.

**Usage**

```
next delete <id>
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--yes` | flag | false | Skip confirmation prompt. |

---

### `next move`

Change the parent of a task. Pass `none` to remove the parent relationship.

**Usage**

```
next move <id> [--parent <id-or-slug>]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--parent <id>` | task ID | — | New parent task. Pass `none` to remove the parent. |
| `--json` | flag | false | Emit the updated task as JSON. |

**Examples**

```sh
next move a1b2 --parent launch-blog
next move a1b2 --parent none
```

---

### `next open`

Open the URL associated with a task in the default browser. Fails with an error if the task has no `url` field.

**Usage**

```
next open <id>
```

---

### `next data set`

Set a key in the task's `data` map. Values are interpreted as JSON: numbers, booleans,
arrays, and objects are stored as their native types; anything that is not valid JSON
is stored as a string. `null` is rejected.

**Usage**

```
next data set <id> <key> <value>
```

Keys must be non-empty, at most 256 characters, and contain only ASCII letters (`a-z`,
`A-Z`), digits (`0-9`), hyphens (`-`), and underscores (`_`). Dots, slashes, and spaces
are not allowed.

**Examples**

```sh
next data set a1b2 source "github"
next data set a1b2 score 42
next data set a1b2 urgent true
```

---

### `next data unset`

Remove a key from the task's `data` map. Errors if the key does not exist.

**Usage**

```
next data unset <id> <key>
```

---

### `next data get`

Print the value of a single key from the task's `data` map. Errors if the key does not exist.

**Usage**

```
next data get <id> <key>
```

---

### `next tag`

Show the current tag state, then list all tags that appear on any task together with any
tags that have a stored description. Tags are grouped for readability into Contexts
(`@`), Resources (`#`) and Freeform — a naming convention, not a difference in
behaviour. Descriptions are shown inline.

**Usage**

```
next tag
```

---

### `next tag rename`

Rename a tag everywhere it appears: every active task, every archived task
(warm `archive/` segments and pruned cold segments alike), the tag's metadata
file, and machine-local state (active/excluded contexts, resource
availability). It is one commit.

Renaming is hierarchical — renaming `@work` also moves `@work/frontend` to
`<new>/frontend`, because a filter on a parent segment matches every
descendant.

The tag's kind must not change: `@context` stays a context, `#resource` stays
a resource, freeform stays freeform. The leading character decides how a tag
filters, so changing it would be a reclassification, not a rename.

By default the rename refuses to run if any destination tag already exists.
`--merge` folds the old tag into the existing one instead: a task carrying
both ends up with a single copy, and where both tags have metadata the
destination's is kept (the source's is dropped, and reported).

**Usage**

```
next tag rename <old> <new> [--merge]
```

**Examples**

```sh
next tag rename @ai/task-manager @ai/next    # also moves @ai/task-manager/*
next tag rename #office #hq
next tag rename py python --merge            # fold py into the existing python
```

A pruned (cold-tier) segment can only be rewritten by restoring it to the
checkout, so a rename that touches one brings it back — exactly as editing a
cold task does. The next archive pass prunes it again.

---

### `next tag describe`

Set a human-readable description for any tag. The description is stored in a per-tag TOML
file under `tags/` and is shown in `next tag` output.

**Usage**

```
next tag describe <tag> <description>
```

**Examples**

```sh
next tag describe @work "Tasks at the standing desk — laptop required"
next tag describe #printer "Office laser printer, 2nd floor"
next tag describe python "Python-related development work"
```

---

### `next tag clear-description`

Remove the stored description for a tag. Errors if no description is set.

**Usage**

```
next tag clear-description <tag>
```

---

### `next tag set-no-time-urgency`

Disable the age and due-date proximity urgency factors for all tasks tagged with `<tag>`.
Useful for "wishlist" or "someday" tags where tasks should not become more urgent simply
because they are old or have a set due date.

**Usage**

```
next tag set-no-time-urgency <tag>
```

---

### `next tag clear-no-time-urgency`

Re-enable time-based urgency for tasks with this tag.

**Usage**

```
next tag clear-no-time-urgency <tag>
```

---

### Other `next tag` subcommands

All tag kinds (`@context`, `#resource`, freeform) share the same metadata store:

```
next tag show <tag> [--json]                 # display all metadata for a tag
next tag set-url <tag> <url>                 # attach a reference URL
next tag clear-url <tag>
next tag set-priority <tag> low|medium|high  # default priority offset for tagged tasks
next tag clear-priority <tag>
next tag data set <tag> <key> <value>        # arbitrary JSON key-value data on a tag
next tag data get <tag> <key>
next tag data unset <tag> <key>
next tag data list <tag>
```

---

### Tag state

Every tag — `@context`, `#resource` or freeform — is in one of three states, and they
work identically for all three kinds. The sigil says what a tag is *for*; it does not
change how the tag filters.

| State | Meaning |
|-------|---------|
| **required** | While anything is required, only tasks carrying a required tag are listed. |
| **excluded** | Tasks carrying it are hidden. Exclusion beats requirement. |
| **accepted** | Neither required nor hidden — and pinning it here stops the tag inheriting a parent tag's state. |

State is inherited down the hierarchy: excluding `#office` also excludes
`#office/printer`. The most specific entry wins, which is what `accepted` is for — it
lets a child opt out of its parent's state. Matching runs downward only: requiring
`@work` covers `@work/frontend`, but requiring `@work/frontend` does not cover plain
`@work`.

The current state is shown at the top of `next tag`.

---

### `next tag require`

Work on these tags. While anything is required, only tasks carrying a required tag are
listed — requiring a tag also hides tasks that carry no tags at all.

Several required tags are a disjunction: "I am at work, or at home".

**Usage**

```
next tag require <tag>...
```

**Examples**

```sh
next tag require @home
next tag require @home @errands
next tag require '#printer'        # only what needs the printer
```

---

### `next tag exclude`

Hide tasks carrying these tags. An excluded tag hides a task even when another of its
tags is required.

**Usage**

```
next tag exclude <tag>...
```

**Examples**

```sh
next tag exclude @work             # not thinking about work today
next tag exclude '#printer'        # the printer is broken
next tag exclude errand chore
```

---

### `next tag accept`

Pin tags to neither required nor excluded. This differs from `clear-state`: an
explicitly accepted tag *stops* inheriting from its parent, where a tag with no entry
inherits.

**Usage**

```
next tag accept <tag>...
```

**Example**

```sh
next tag exclude @home             # not doing home tasks…
next tag accept @home/kitchen      # …except in the kitchen
```

---

### `next tag clear-state`

Drop the stored state for these tags, so they inherit from their parents again. With no
arguments, clears every tag's state.

**Usage**

```
next tag clear-state [<tag>...]
```

**Examples**

```sh
next tag clear-state @home
next tag clear-state              # back to showing everything
```

---

### `next user`

Show the currently active user filter.

**Usage**

```
next user
```

---

### `next user set`

Set the global active user filter. Tasks assigned to users not in this set are hidden.
Unassigned tasks are always visible.

**Usage**

```
next user set <name>...
```

---

### `next user clear`

Clear the user filter.

**Usage**

```
next user clear
```

---

### `next user list`

List all usernames that appear as `assignee` on any task. Marks active users with `*`.

**Usage**

```
next user list
```

---

### `next forecast`

Show upcoming recurrence due dates for all matching recurring tasks, projected over a configurable horizon (default: 90 days). Accepts the same filter tokens as `next list`.

Both recurrence modes are projected. Schedule-mode dates are exact — the RRULE says when
they fall. Completion-mode dates are a **best case**: future completion dates are unknown,
so the projection assumes you finish each instance on its due date. Complete one late and
the real series slips behind the forecast. The two are not distinguished in the output.

**Usage**

```
next forecast [options] [filters...]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--days <N>` | positive integer | 90 | Forecast horizon in days. Defaults to `forecast_horizon_days` in config. |
| `--all` | flag | false | Disable all implicit filtering. |
| `--all-users` | flag | false | Bypass the user filter. |
| `--json` | flag | false | Emit forecast as JSON. Alias for `--format json`. |
| `--format <fmt>` | `table` \| `json` | `table` | Output format. |
| `--count` | flag | false | Print only how many entries the forecast contains. An entry is one occurrence on one date, so a weekly task inside the horizon contributes several. |

Flags precede the filter here too. There is no `--fields`: `--fields` projects a
task object, and a forecast entry is a date, an id and a title rather than a
task — there is nothing to project it down to.

`--future` is implied and not offered: a forecast that hid future-start tasks
would have nothing to show.

---

## Recurrence

`next` supports two recurrence modes. In both cases, marking a task done with `next done` automatically creates the next instance and commits both changes in a single git commit.

### Schedule-based (`--recur-schedule`)

The next instance is determined by an RFC 5545 RRULE string. The `anchor` date (the first `start` or `due` date, or today if neither is set) pins the series so that multi-interval rules stay aligned over time.

**Supported RRULE fields**

The rule is handed to the `rrule` crate, so the whole of its RFC 5545 surface works — this table lists the parts you are likely to reach for, not the limit of what is accepted.

| Field | Example | Notes |
|-------|---------|-------|
| `FREQ` | `FREQ=WEEKLY` | Required. `DAILY`, `WEEKLY`, `MONTHLY`, `YEARLY`. |
| `INTERVAL` | `INTERVAL=3` | Every Nth period. Default 1. Must be at least 1. |
| `BYDAY` | `BYDAY=MO,TU,WE,TH,FR` | Comma-separated weekday codes (`MO TU WE TH FR SA SU`). When omitted in a weekly rule, defaults to the anchor's weekday. Positional prefixes are supported: `1MO` is the first Monday of the period, `-1FR` the last Friday. |
| `BYMONTHDAY` | `BYMONTHDAY=1` | Day of month, 1–31 or -1–-31. Negative counts back from the end, so `-1` is the last day of the month. Must not be `0`. |
| `BYMONTH` | `BYMONTH=1` | Month number, 1–12. Needed to pin a `FREQ=YEARLY` rule to one month. |
| `BYSETPOS` | `BYDAY=MO;BYSETPOS=-1` | Selects from the occurrences a period generates — here, the last Monday of the month. |
| `UNTIL` / `COUNT` | `UNTIL=20261231T000000Z`, `COUNT=12` | End the series on a date or after N occurrences. Once exhausted, `next done` marks the last instance done and spawns nothing. |
| `WKST` | `WKST=SU` | Week start, which changes how `INTERVAL` groups weeks. Default `MO`. |

**Months that skip**

A day that does not exist in a month is **skipped**, not clamped back — this is what RFC 5545 requires. `FREQ=MONTHLY;BYMONTHDAY=31` runs Jan 31, Mar 31, May 31 … with no February occurrence at all. If you want the last day of *every* month, ask for it directly:

```sh
--recur-schedule "FREQ=MONTHLY;BYMONTHDAY=-1"
```

**Common patterns**

```sh
# Every weekday (Mon–Fri)
--recur-schedule "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"

# Every Monday (BYDAY omitted → defaults to anchor's weekday)
--recur-schedule "FREQ=WEEKLY"

# Every Monday (explicit)
--recur-schedule "FREQ=WEEKLY;BYDAY=MO"

# 1st of every month
--recur-schedule "FREQ=MONTHLY;BYMONTHDAY=1"

# Every 3 months on the 1st (quarterly)
--recur-schedule "FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1"

# Every year on Jan 1 — BYMONTH is required, or the rule fires on the 1st of every month
--recur-schedule "FREQ=YEARLY;BYMONTH=1;BYMONTHDAY=1"

# First Monday of every month
--recur-schedule "FREQ=MONTHLY;BYDAY=1MO"

# Last day of every month
--recur-schedule "FREQ=MONTHLY;BYMONTHDAY=-1"

# Every Monday for a year, then stop
--recur-schedule "FREQ=WEEKLY;BYDAY=MO;COUNT=52"
```

When a task has both `start` and `due` dates, the start-to-due offset is preserved on every new instance. For example, a task with start=June 1, due=June 3 will next appear as start=July 1, due=July 3.

If you complete a task late (past its due date), the next occurrence is computed from `today` rather than from the original due date, so the series never schedules a date that has already passed.

### Completion-based (`--recur-completion <days>`)

The next instance is created `<days>` after the completion date (i.e., relative to when you mark it done, not a fixed calendar date). `<days>` must be at least 1: a zero-day interval would never advance the series, so it is rejected — the same way `INTERVAL=0` is rejected in an RRULE.

```sh
next add "Water plants" --recur-completion 7   # once a week, whenever done
next add "Dentist check" --recur-completion 180 # every ~6 months
```

### Snap values (`--recur-snap`)

Moves the computed next date to a qualifying day. Use when you want to round to a convenient boundary.

| Value | Boundary |
|-------|----------|
| `monday` … `sunday` | That weekday (the date is kept if it is already there). |
| `next-workday` | Any Mon–Fri. |
| `dom:N` | Day N of the month (N = 1–28, so the day exists in every month). |

```sh
# Completion-based, snapped to Saturday
next add "Weekly chore" --recur-completion 7 --recur-snap saturday

# Monthly on the 1st, snapped to next workday if the 1st is a weekend
next add "Monthly report" --recur-schedule "FREQ=MONTHLY;BYMONTHDAY=1" --recur-snap next-workday
```

By default a snap only ever moves a date **later**, and by as much as it takes to reach the next boundary. That is what [Snap leeway](#snap-leeway---recur-snap-leeway) exists to bound.

### Snap leeway (`--recur-snap-leeway`)

A bare snap is a ratchet, not a rounding. Take a rent task set up the way this page suggests — `--recur-completion 30 --recur-snap dom:1` — and complete it on the 2nd instead of the 1st. The raw date is the 2nd of next month, the 1st has already gone past, so the snap jumps a further month: the cycle you asked to be 30 days long becomes 60. Nothing warns you, and completing one day late every time halves the number of payments in a year.

`--recur-snap-leeway` turns the boundary into a **tolerance**. The raw date moves to a boundary only if one falls within the window; otherwise the raw date stands and the interval is preserved exactly. Keeping the raw date is the feature, not a fallback — an off-boundary date on cadence beats an on-boundary date a month late.

| Completed | Raw (+30) | Default → due | Gap | `--recur-snap-leeway 3` → due | Gap |
|-----------|-----------|---------------|-----|-------------------------------|-----|
| 2026-06-01 | 2026-07-01 | 2026-07-01 | 30 | 2026-07-01 | 30 |
| 2026-06-02 | 2026-07-02 | 2026-08-01 | 60 | 2026-07-01 | 29 |
| 2026-06-05 | 2026-07-05 | 2026-08-01 | 57 | 2026-07-05 | 30 |
| 2026-06-15 | 2026-07-15 | 2026-08-01 | 47 | 2026-07-15 | 30 |
| 2026-06-30 | 2026-07-30 | 2026-08-01 | 32 | 2026-08-01 | 32 |

Read the third and fourth rows: with a leeway of 3 the 1st is out of reach, so the date stays on the 5th and the 15th. The task is off its boundary but on its cadence. Note also that the second row lands a day *short* of 30 — a backward pull can shorten one cycle, by at most `BACK` days, and the next cycle starts from the boundary again.

**The spec**

| Form | Meaning |
|------|---------|
| `N` | Both directions. `--recur-snap-leeway 3` is back 3, forward 3. |
| `BACK,FORWARD` | Independent. `--recur-snap-leeway 5,0` pulls back up to 5 days and never pushes later. |
| `BACK,*` | Forward unbounded. `--recur-snap-leeway '5,*'` pulls back up to 5 days and pushes later however far it takes — the default's forward behaviour, with a backward tolerance added. Quote it: the `*` belongs to the spec, not to the shell. |

Whole days only, each between 0 and 365, or `*` for an unbounded forward direction. `0` means the corresponding direction never moves the date; `0,0` never snaps at all, and `0,*` is exactly the default (which you would normally spell by leaving the flag off).

`*` exists so that every leeway the file format can hold has a spec string. A hand-written `[recurrence.snap_leeway]` may omit `forward`, and the TUI edit form renders a stored leeway back into this grammar to seed its Leeway row — without a spelling for "unbounded" the form would read such a task back as bounded and quietly rewrite it on save. (`next show` describes the leeway in prose instead, so it was never affected.)

**The rule**, in the order it is applied:

1. If the raw date already sits on a boundary, keep it.
2. If a boundary is within `BACK` days, pull back to it. (For a completion rule it must also stay after the date you completed on; a schedule rule handles that further down, by discarding whole occurrences.)
3. If a boundary is within `FORWARD` days, push forward to it.
4. If both are in range, the nearer wins; a tie goes forward.
5. If neither is in range, keep the raw date.

**The default is not "no snapping"**

Omitting `--recur-snap-leeway` means `back 0, forward unbounded` — precisely the behaviour every task had before this flag existed. No stored task changes date, no file is rewritten, nothing migrates. It also means the default is the ratchet described above, so `next add` prints a hint when you set a `dom:N` or weekday snap without one.

**Small leeways and `next-workday`**

`next-workday` sits one day back from Friday and two days forward to Monday when the raw date is a Saturday, and the mirror when it is a Sunday. So a Saturday only reaches Monday if `FORWARD` is at least 2, and a Sunday only reaches Friday if `BACK` is at least 2. Consequences worth knowing before you type a small number:

| Leeway | Raw falls on a Saturday | Raw falls on a Sunday |
|--------|-------------------------|-----------------------|
| unset (default) | Monday | Monday |
| `2` | Friday (nearer than Monday) | Monday (nearer than Friday) |
| `1` | Friday | Monday |
| `0,1` | **stays on Saturday** | Monday |
| `1,0` | Friday | **stays on Sunday** |
| `0,0` | **stays on Saturday** | **stays on Sunday** |

A weekend date that simply stands is the rule working as designed — no boundary was close enough — but it reads like a bug if you were not expecting it. `next show` prints the leeway alongside the snap so the cause is visible.

**Errors**

| Situation | Message |
|-----------|---------|
| Leeway with no snap | `--recur-snap-leeway requires a snap; set --recur-snap first (e.g. dom:1, monday, next-workday)` |
| Backward tolerance not shorter than the interval | `backward leeway (5d) must be less than the completion interval (3d), or the series would not advance` |
| Out of range | `snap leeway must be between 0 and 365 days, got 400` |
| Malformed | `invalid snap leeway "3,-1" — expected N, BACK,FORWARD or BACK,* in whole days (e.g. 3, 5,0 or 5,*)` |
| Set and clear together | `--recur-snap-leeway and --clear-recur-snap-leeway are mutually exclusive` |

The backward bound applies to completion rules, where the interval is known. A schedule rule has no static period to compare against, so it is guarded differently: the RRULE's occurrences are snapped one by one and the first date that lands strictly *after* the current instance's is the one used. An occurrence that a backward pull moves onto a date already held is stepped over, not spawned again — so a weekly rule spawns once a week however wide the tolerance, and the pull can only ever shorten a single cycle.

The same walk drives `next forecast`, so what the forecast shows for a schedule series is what `next done` will really spawn.

```sh
# The rent fix, end to end
next add "Pay rent" --slug rent --due 2026-06-01 \
  --recur-completion 30 --recur-snap dom:1 --recur-snap-leeway 3

# Weekly on Monday, tolerating a two-day slip either way
next add "Weekly review" --recur-completion 7 --recur-snap monday --recur-snap-leeway 2

# Asymmetric: pull back up to 5 days, never push past the 1st
next edit rent --recur-snap-leeway 5,0
```

### Managing recurrence with `next edit`

Use `next edit` to change recurrence settings on an existing task:

```sh
next edit standup --recur-snap monday          # change snap without re-specifying the rule
next edit standup --recur-schedule "FREQ=DAILY" # change the rule; anchor is preserved
next edit standup --clear-recurrence           # remove the rule entirely
```

When you change a schedule rule the `anchor` date is **preserved**, keeping all interval calculations aligned.

### Series identity

All instances of the same recurring task share a `recurrence_id` UUID equal to the first instance's `id`.

There is no way to query a series by it: the filter language has no `recurrence_id` atom and `--fields` cannot project the field, so `recurrence_id` is currently an internal link only. `is:recurring` selects tasks that carry a rule, which is not the same thing — it matches the current instance of every series, not every instance of one.

Slugs are **not** propagated — each instance gets no slug. This prevents slug collisions on high-frequency tasks. Arbitrary data set with `next data set` is copied to each new instance (the `time_log` entry is not, since it records per-instance work time).

---

### `next sync`

Synchronise with the remote git repository: pull, reconcile the local cache, run the
automatic archive pass if due, then push local commits. Stops with a detailed error
message if a merge conflict is detected. After a clean sync, any registered plugin
whose periodic sync is due is run (see `next plugin set-sync`).

**Usage**

```
next sync [options]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--push-only` | flag | false | Only push local commits; skip pulling from the remote. |
| `--pull-only` | flag | false | Only pull from the remote; skip pushing. |

---

### `next plugin`

Manage export plugins. Registrations are machine-local (stored in the per-repo
`state.toml` under `$XDG_STATE_HOME`, never committed). A plugin has two optional
commands: the **export hook** (`register`), spawned fire-and-forget whenever a watched
task changes, and the **periodic sync** (`set-sync`), run to completion after each
`next sync` when its interval has elapsed.

**Usage**

```
next plugin register <name> -- <program> [args…]   # define/replace the export command
next plugin watch <name> <task>                    # notify <name> on updates to <task>
next plugin unwatch <name> <task>
next plugin unregister <name>                      # remove plugin + subscriptions
next plugin set-sync <name> [--default-interval <secs>] -- <program> [args…]
next plugin set-interval <name> <secs>             # user override of the sync interval
next plugin set-interval <name> --clear            # drop the override
next plugin enable <name> | disable <name>         # toggle the periodic sync
next plugin list
```

The sync interval resolves as: `set-interval` override → `--default-interval` →
`sync.plugin_sync_default_secs` in `config.toml` (default 86400 = daily). Only a
successful sync run records `last_sync`; failures are retried on the next sync.

---

### `next config`

Read or write a value in the machine-local `config.toml` without opening a repository.

**Usage**

```
next config get [<key>]          # print one key, or all known keys when omitted
next config set <key> <value>
```

Supported keys: `repository`, `list_limit` (`none` clears), `next_count`,
`forecast_horizon_days`, `sync.git_subprocess`, `sync.autopull`, `sync.autopush`,
`sync.staleness_secs`, `sync.pull_timeout_secs`.

**Examples**

```sh
next config get                                # print everything
next config set sync.autopush true
next config set sync.autopull false
next config set list_limit none
```

---

## Filter Syntax

All list commands (`list`, `next`, `tree`, `forecast`) accept a filter
expression. The trailing arguments are joined with a space and parsed as one
query, so `next list +@work -bug` and `next list '+@work -bug'` mean the same
thing — only phrases, parentheses and `#resource` tags need shell quoting.

> **A bare word searches; it is not a tag.** `next list bug` looks for "bug" in
> the title, description, notes and url. To select tasks *tagged* `bug`, write
> `next list +bug`. Every sigil form is unchanged.

The expression is the **trailing** part of the command line, and every flag goes
before it: `next list --all +@work`, never `next list +@work --all`. A flag
written after the expression is rejected with a message saying so. The trailing
arguments have to be taken verbatim — that is what keeps `-bug` and `-#printer`
usable as exclusions instead of being read as unknown short flags — and taking
them verbatim leaves no way to tell a trailing `--all` from a term. Refusing is
the only honest answer. Exclusions themselves need no change: they were always
part of the expression, and they stay where they are.

### Token reference

| Token | Example | Meaning |
|-------|---------|---------|
| `<word>` | `bug`, `"cold tier"`, `arch*` | Full-text search over title, description, notes and url. A quoted phrase matches consecutive words in order; a trailing `*` matches by prefix. Matching is by whole word, so `arch` does not match "archive". |
| `<field>:<word>` | `title:rebuild`, `notes:sqlite` | The same search, restricted to one text field. |
| `+<tag>` | `+python`, `+@home`, `+#printer` | Task must have this tag (or one nested under it: `+@work` matches `@work/backend`). |
| `-<tag>` | `-@work`, `-reading` | Task must not have this tag. |
| `<field>:<value>` | `status:open`, `priority:high`, `assignee:alice`, `slug:water-plants`, `data.estimate:3` | Field equality. A comma-separated list is a set: `status:open,started`. `slug:` is exact — it does not match by prefix. |
| `id:<id>` | `id:a1b2c3d4`, `id:a1b2c3d4-e5f6-0718-293a-4b5c6d7e8f90` | Task has this id. Matches by **prefix**, so the eight characters a listing prints are enough; four is the minimum. Hyphens are optional, case is ignored. |
| `<field><op><value>` | `due<+7d`, `priority>=medium`, `created>2026-08-01` | Ordered comparison (`<`, `<=`, `>`, `>=`) over `priority`, the dates, and numeric `data.*`. |
| `<field>:<low>..<high>` | `due:2026-08-01..eom` | Inclusive range. |
| `has:<field>` / `no:<field>` | `has:due`, `no:assignee`, `has:context` | Whether the field is set. `has:context` asks whether the task carries any `@` tag. |
| `is:<name>` | `is:overdue`, `is:blocked`, `is:project`, `is:recurring`, `is:archived`, `is:closed`, `is:assigned` | Named predicates. `is:archived` reports whether the task is in the archive tier (true for every row of an `--archived` listing, false otherwise) — it does not search the archive from a non-archived query. See the note below on `is:project` and `is:blocked`. |
| `and` `or` `not` `( )` | `+@work and (due<+7d or is:overdue)` | Booleans, also spelled `&`, `|`, `!`. Adjacency means `and`, and precedence runs `not` > `and` > `or`. Quote an operator word (`"or"`) to search for it literally. |
| `parent:<slug>` | `parent:work`, `parent:launch-blog` | Task is a descendant (direct or transitive child) of the task with this slug. Exactly one slug — not a set. |
| `context:<@tag>` | `context:@home` | Include exactly this tag for this query only, ignoring whatever the stored state includes. Exclusions still apply. |
| `user:<name>` | `user:alice` | Override the global user filter for this query only. Survives `--all`. |

The three flags below are not tokens — they widen the pool of candidate tasks
rather than describing one — so they go before the expression like every other
flag:

| Flag | Meaning |
|------|---------|
| `--future` | Include tasks with a future `start` date. |
| `--all` | Disable all implicit filtering. |
| `--all-users` | Bypass the user filter only. |

### Implicit filtering (default behaviour)

Unless `--all` is passed, the following tasks are always excluded:

- Tasks with `status` other than `open` or `started`
- Tasks whose `start` date is in the future
- Tasks that are blocked (any open `blocked_by` entry, or the parent of any open subtask)
- Tasks whose `assignee` does not match the active user set (tasks with no `assignee` are always shown)

The **tag state** is applied separately and is *not* disabled by `--all`, which widens
the statuses shown rather than the tags: a tag excluded on purpose stays excluded until
it is un-excluded. Tasks carrying an excluded tag are hidden, and while any tag is
required, tasks that carry no required tag are hidden too.

`--all` drops the *stored* user filter along with the rest of the gate, but it does
not drop a `user:` term you wrote yourself. `--all` means "stop applying the
defaults", not "ignore what I asked for", and a query that named alice and came
back with bob's tasks would be the tool overruling its user. So
`next list --all user:alice` is alice's work in every status; `--all-users` is the
way to ask for every assignee, and it still wins over a `user:` term when both are
given.

### How search matches

Search is **word-oriented**, not substring: text is split on non-alphanumeric
characters and lowercased, and a term matches whole words.

| You type | Matches | Does not match |
|----------|---------|----------------|
| `arch` | "arch" | "archive", "search" |
| `arch*` | "arch", "archive", "archiving" | "research" |
| `"cold tier"` | "the cold tier segment" | "tier cold", "cold storage tier" |
| `café` | "Café", "CAFÉ" | "cafe" |

Case is ignored; accents are **not**, so `café` and `cafe` are different words.
A term made only of punctuation (`"---"`) matches nothing — there is no word in
it to look for.

The rules above hold whether or not the SQLite cache is present. The cache
answers an ASCII search from a full-text index; a non-ASCII term, or a store
with no cache, scans the text directly. Both give the same answer — the split
exists because the index and the scan only tokenise identically over ASCII, so
anything else takes the path that is definitive rather than the one that is
fast.

Searching reaches archived tasks too, including segments pruned out of the
working tree that `grep` cannot see.

### A status predicate can contradict the gate

The implicit gate runs *before* the expression and keeps only open and started
tasks. A query that asks for a closed one is therefore asking for something that
is already gone:

```sh
next list status:done            # empty: the gate dropped the done tasks first
next list is:closed              # empty, for the same reason
next list --closed status:open   # empty from the other side: --closed keeps only closed tasks
```

These are the first queries most people try after reading the grammar table, and
none of them is wrong — the predicates mean exactly what they say. They are simply
evaluated over a pool the gate has already emptied of the tasks in question. A
listing that spots the contradiction prints a hint naming `--all`; **which tasks
match is unchanged**, the hint is only there to shorten the confusion.

The spellings that work:

```sh
next list --all status:done      # every status is a candidate; the query picks done
next list --closed               # the closed listing, most recently completed first
```

### `is:project` and `is:blocked` interact with the implicit gate

Both ask about *other* tasks, so the default list — which hides a parent while
any child is open, and hides a task while any blocker is open — removes most of
what they select before the query is even evaluated:

- `next list is:project` lists only projects whose children are **all**
  resolved. A project with an open subtask is hidden by the gate, because the
  advice is to work on the subtask.
- `next list is:blocked` lists nothing, since a blocked task is hidden by the
  gate for the same reason.

Add `--all` to see them: `next list --all is:blocked` is the useful spelling.
The predicates themselves are exact — the gate is what narrows the view.

### Dates in a filter

An unquoted date value takes the compact forms: ISO (`2026-08-10`), a signed
offset (`+7d`, `-2w`, `+3m`, `-1y`), a named day (`today`, `tomorrow`,
`yesterday`) or an end-of-period (`eow`, `eom`, `eoy`). Quote the value to use
natural language: `due:"next monday"`.

A field that is unset never compares true — a task with no deadline is not
matched by `due<+7d` *or* by `due>+7d`.

### Filtering the archive

`next list --archived` takes the same grammar as the active list, over rows the
cache holds for every archived task — including segments that have been pruned
out of the checkout entirely, which `grep` cannot reach.

Five terms are refused there rather than silently ignored, because an archive
listing has neither a view of other tasks nor git history to consult:

| Term | Why |
|------|-----|
| `is:blocked`, `is:project`, `parent:` | They are questions about *other* tasks. |
| `created:`, `updated:` | They come from git history, which this listing does not load. |
| `context:`, `user:` | They scope the whole view, which this listing does not apply. |

### The fields

| Field | Values | Ordered? |
|-------|--------|----------|
| `status` | `open`, `started`, `done`, `cancelled` | no |
| `priority` | `low`, `medium`, `high` | yes — `low < medium < high` |
| `due`, `start`, `completed` | a date (see below) | yes |
| `created`, `updated` | a date, from git history | yes |
| `assignee` | a username | no |
| `user` | a username; also matches **unassigned** tasks | no |
| `slug` | exact match, not a prefix | no |
| `parent` | a slug — scopes the query to that subtree | no |
| `tag`, `context` | a tag; `tag:x` is the long form of `+x` | no |
| `title`, `description`, `notes`, `url` | scope a search to one field | no |
| `data.<key>` | a task-data value; compares by its stored JSON type | yes, when numeric |

Ordered fields take `<`, `<=`, `>`, `>=` and `low..high`; most fields take `:`
and a comma-separated set (`status:open,started`). A field that is unset on a
task never compares true — a task with no deadline matches neither `due<+7d`
nor `due>+7d`.

`parent:` is the exception: it takes exactly one slug. It does not describe a
task, it scopes the whole view to one subtree, and two subtrees are not a view —
so `parent:a,b` is refused with that explanation rather than quietly using the
first slug and dropping the second.

An unknown field name is an error rather than a fallback to search, and the error
names the way out: quote the token to search for it literally. A pasted URL is
the common case, since `https://example.com/x` reads as the field `https` —

```sh
next list '"https://example.com/x"'
```

The same applies to any accidental `foo:bar`. Falling back to a search would be
friendlier in the moment and wrong in the long run: a mistyped `stauts:open`
would silently become a full-text search for that string and return nothing,
which looks exactly like "no tasks match".

### Precedence, quoting and reserved words

Precedence runs `not` > `and` > `or`, and adjacency means `and` — so
`a or b c` is `a or (b and c)`. Parentheses override it.

`and`, `or` and `not` are reserved: they are read as operators wherever a term
could go. To search for one as a word, quote it — `"or"`. The symbols `&`, `|`
and `!` are the same operators, and `&|!()"` cannot appear in a bare term for
that reason; quote a term that needs them.

Nesting is limited to 32 levels. That is far past any hand-written query and
exists because the parser recurses: without a bound, a deeply nested expression
would overflow the stack, which aborts the process rather than raising an error
the tool could report.

### Choosing which fields come back

`--fields` trims the JSON output to the fields you name. Every command that
emits tasks takes it — `next list`, `next next`, `next tree` and `next show` —
and so do the MCP `list_tasks` and `get_task` tools:

```sh
next list --json --fields id,title,due
next show rebuild-cache --json --fields id,title,status
```

The driver is payload size, not tidiness. A listing serialises every field of
every task, notes and descriptions included, and the cost is per row — so
pagination does not help and projection does.

The names are the same ones the filter grammar uses, so `due` means the same
thing in `--fields due` as in `due<+7d` — and `id` names the same thing in
`--fields id` as in `id:a1b2c3d4`, so you can filter on what you project.
`data.<key>` picks one entry out of the task data; `data` takes the whole map.
Two spellings for one concept is how a tool becomes hard to learn, so the only
additions are `score` and `score_breakdown`, neither of which is a field of a
task.

A field you did not ask for is **absent** from the object, not `null` — `null`
already means "this task has no due date", and a consumer could not tell the
two apart otherwise.

Projection does not change which tasks match: `--fields id` with a filter on
`notes:x` still filters on notes. The default returns every field, so this
changes nothing until you ask for it.

**It applies to JSON only.** `--fields` without `--json` (or `--format json`) is
a usage error — *"`--fields` applies to JSON output; add `--json` (the table
prints a fixed set of columns)"* — not a silent no-op. The table has a fixed set
of columns and cannot honour the request, and a flag that appears to work while
doing nothing is the worse of the two failures. `list`, `show`, `tree` and
`next` all refuse it in the same words.

**It does not combine with `--count`.** `--count` prints a number and no task,
so there is nothing left to project; `next list --json --count --fields id`
satisfies the JSON rule and would still have printed a bare count with the
projection dropped. It is refused for the same reason as the case above.

**`created` and `updated` cannot be projected.** They can be *filtered*
(`created>2026-08-01`), because the listing reads them out of git history for
exactly that purpose, but they are not part of the task object — there is
nothing for a projection to keep. Naming one is refused with that explanation
rather than returning an object quietly missing a field you believe you asked
for.

**`score` is not a task field either**, but for the opposite reason: it is
computed, and it sits *beside* the task in the response rather than inside it.
So `--fields score` on its own gives you the score next to an empty task object,
which is almost never the intent — `--fields id,score` is what you want, and
`--fields id,title,score` is the usual shape. `score_breakdown` works the same
way, and on `next show --json` both come back only when named.

### Explaining a query

`next list --explain` shows what a filter became instead of running it. Use it
when a query returns something unexpected:

```sh
next list --explain bug
```

```
Query:    bug
Parsed:   bug

Read as SEARCHES (text, not tags):
  bug
  A bare word searches the title, description, notes and url.
  For the TAG of that name, write +bug.

Execution:
  pushed into SQL:   the status gate only
  the filter itself: evaluated in memory
  candidates loaded: 12
  matched:           0

Nothing matched. The implicit gate hides closed tasks, future
start dates, blocked tasks and parents with open subtasks —
try --all to see past it.
```

The search warning is deliberately quiet, because a diagnostic that cries wolf
is one people stop reading. It appears only when the search was **unscoped** —
`title:bug` already says out loud that it is a text search, so there is nothing
to point out — and it offers the `+bug` spelling only when `bug` is a tag that
actually exists in this repository. Suggesting a tag nobody has ever created
would send the reader off to debug a query that was never going to work.

It also reports the terms that scope the whole view (`parent:`, `context:`,
`user:`), and says so plainly when no filter arrived at all — usually a sign
the shell consumed it.

Over the archive (`next list --archived --explain`) the execution block reports
the compiled SQL `WHERE` clause instead, and whether SQL owned the answer or
merely narrowed the rows for a re-check in memory. When it merely narrowed them
the candidate line reads:

```
  candidates loaded: not counted (the store re-checks the rows and reports only the matches)
```

The rows *are* all read — an inexact pushdown fetches every candidate, re-checks
each one in memory and paginates the survivors — but the store hands back only
the survivors and their total. The number of rows it looked at on the way is not
part of that answer, so the explanation cannot report it, and inventing one is
the single thing an explanation must not do. That number is exactly the one
worth having, too: the gap between it and `matched` is how far the pushdown
over-selected. Reporting the match count under both labels would have hidden
that gap behind a figure that always agreed with itself.

When the pushdown *is* exact, SQL answered the query outright, every candidate
is a match, and the two lines genuinely are one number — so `candidates loaded`
prints it.

The closing hint drops its
`--all` suggestion there too: `--archived` and `--all` cannot be combined, and
advice that the argument parser would reject is worse than no advice.

The explanation is prose, aimed at a person, and there is deliberately **no
JSON form of it**: a machine-readable explanation would be a second contract to
keep in step with the pipeline, and an explanation that can disagree with the
pipeline is worse than none. `--json`, `--format json`, `--fields` and `--count`
therefore have nothing to act on alongside `--explain`, and combining them is
refused rather than silently resolved in `--explain`'s favour.

### Not yet supported

- `score` cannot be filtered on: a task's score is computed after filtering. It
  *can* be projected — see [Choosing which fields come back](#choosing-which-fields-come-back) —
  because projection happens after scoring, not before.

---

## Design Notes

### Task ID references

Task IDs are UUID v4 values. On the command line, any unambiguous prefix of at least 4
hex characters is accepted. Slugs are also accepted wherever an ID is expected.

The filter grammar's `id:` follows the same four-character floor, with one
difference that follows from what a filter is: an ambiguous prefix is not an
error there, it simply matches every task it names. A command that acts on one
task has to refuse to guess which; a listing has no such problem.

### Projects and subtasks

Any task with children is a project — there is no separate project entity and no special
tag or type is required. Child tasks attach via `--parent`. A parent task is hidden from
the default scored list while any of its direct children are still open.

Use `next tree` to see all tasks in their parent-child structure. Use `next show <id>`
to inspect a single task and its direct children.

### Tag descriptions

Descriptions are stored as individual TOML files under the `tags/` directory.  `@` and `#`
prefixes are encoded on disk (`@` → `__context__`, `#` → `__resource__`) so paths are
safe on all platforms: `@work` → `tags/__context__work.toml`, `#printer` →
`tags/__resource__printer.toml`, `@home/kitchen` → `tags/__context__home/kitchen.toml`.
`next tag describe` and `next tag clear-description` are the canonical way to manage
descriptions for all tag types; the encoding is transparent to the user.

Repositories that still contain the old `[tag_descriptions]` table in `state.toml` are
migrated automatically on first open: each entry is written to its own `tags/*.toml`
file and the table is removed from `state.toml`.

### `data` field

The `data` map stores arbitrary key-value pairs (strings, numbers, booleans — null is
not allowed). Intended for AI-provided metadata and tool integrations. Keys are validated:
non-empty, at most 256 characters, and may only contain `a-zA-Z0-9_-`.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | User or input error (bad arguments, ambiguous ID, etc.) |
| `2` | System error (git conflict, network failure, DB error, etc.) |
