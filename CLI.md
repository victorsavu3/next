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
| `next tag include` | Work on these tags: only their tasks are listed |
| `next tag exclude` | Hide tasks carrying these tags |
| `next tag default` | Pin tags to no state, ignoring a parent tag's state |
| `next tag clear-state` | Drop the stored state for tags (all of them when none given) |
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
| `--config <path>` | Config file to use (default: `$XDG_CONFIG_HOME/task-manager/config.toml`). |
| `--repo <path>` | Task repository root. Overrides `repository` in the config and the upward `.git` search. |
| `--autopull` / `--no-autopull` | Force the pre-command staleness pull on or off for this invocation. Overrides `sync.autopull` in config. |
| `--autopush` / `--no-autopush` | Force the post-mutation push on or off for this invocation. Overrides `sync.autopush` in config. |
| `--autosync` | Master switch: enable **both** autopull and autopush for this invocation. |
| `--no-autosync` | Master switch: disable **both** autopull and autopush for this invocation. |
| `--offline` | Alias for `--no-autosync`: disable both — no network I/O. |

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
| `--recur-completion <days>` | positive integer | none | Completion-based recurrence. The next instance is created `<days>` after the task is marked done. |
| `--recur-snap <snap>` | snap value | none | Advance the computed next date to the nearest qualifying date. See [Snap values](#snap-values) below. Applies to both schedule and completion modes. |
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
next add "Water plants" --slug water-plants --recur-completion 7 --recur-snap saturday --tag @home

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
next list [filters...]
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
| `--json` | flag | false | Emit the result as JSON: `{ "items": [...], "page": N, "page_size": N, "total": N }`. |

Filter tokens (see [Filter Syntax](#filter-syntax)) may be placed anywhere in the argument list.

A default cap can be set in config as `list_limit = N`; without one the page
size defaults to 50. When the result is a window on a larger set, text output
ends with an indication such as `page 2 of 14 · 13402 matching · --page 3 for more`.

**Examples**

```sh
# Default view (respects the tag state)
next list

# All Python-tagged tasks
next list +python

# Tasks available at home, including those not yet started
next list context:@home --future

# Everything, bypassing all implicit filters
next list --all

# Archived work-tagged tasks, second page
next list --archived +@work --page 2
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
next next [N] [filters...]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `[N]` | positive integer | 10 | Number of tasks to show. |

**Options**

Same filter flags as `next list`.

**Examples**

```sh
next next
next next 5 context:@work
next next --json
```

---

### `next show`

Display the full details of a single task, including its description, URL, data, notes, all tags, subtask list, blockers, recurrence configuration, and urgency score breakdown (due, priority, age, tag, and other factors shown inline). A closed (done or cancelled) task scores 0 and shows no breakdown.

**Usage**

```
next show <id>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID, slug, or any unambiguous prefix (minimum 4 hex characters). |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit full task details as JSON. |

---

### `next tree`

Show all tasks in a parent-child tree. Root tasks (no parent) are listed at the top;
child tasks are indented under their parent.

**Usage**

```
next tree [options]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--all` | flag | false | Include done and cancelled tasks. Default shows open tasks only. |
| `--json` | flag | false | Emit a flat task list as JSON (with `parent_id` fields). |

**Examples**

```sh
# Tree of open tasks
next tree

# Full tree including completed work
next tree --all
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
| `--json` | flag | false | Emit the completed task (and the new recurrence instance, if any) as JSON. |

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
| `--json` | flag | false | Emit the updated task as JSON. |

`--recur-schedule`, `--recur-completion`, and `--recur-snap` work the same as in `next add`. When editing a schedule rule, the original `anchor` date is preserved so interval alignment stays correct. `--recur-snap` can also be used standalone to change the snap on an existing recurring task without re-specifying the full rule.

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
| **included** | While anything is included, only tasks carrying an included tag are listed. |
| **excluded** | Tasks carrying it are hidden. Exclusion beats inclusion. |
| **default** | No state — and pinning it here stops the tag inheriting a parent tag's state. |

State is inherited down the hierarchy: excluding `#office` also excludes
`#office/printer`. The most specific entry wins, which is what `default` is for — it lets
a child opt out of its parent's state. Matching runs downward only: including `@work`
covers `@work/frontend`, but including `@work/frontend` does not cover plain `@work`.

The current state is shown at the top of `next tag`.

---

### `next tag include`

Work on these tags. While anything is included, only tasks carrying an included tag are
listed — including a tag also hides tasks that carry no tags at all.

Several included tags are a disjunction: "I am at work, or at home".

**Usage**

```
next tag include <tag>...
```

**Examples**

```sh
next tag include @home
next tag include @home @errands
next tag include '#printer'        # only what needs the printer
```

---

### `next tag exclude`

Hide tasks carrying these tags. An excluded tag hides a task even when another of its
tags is included.

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

### `next tag default`

Pin tags to no state. This differs from `clear-state`: an explicitly defaulted tag
*stops* inheriting from its parent, where a tag with no entry inherits.

**Usage**

```
next tag default <tag>...
```

**Example**

```sh
next tag exclude @home             # not doing home tasks…
next tag default @home/kitchen     # …except in the kitchen
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

**Usage**

```
next forecast [filters...] [--days <N>]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--days <N>` | positive integer | 90 | Forecast horizon in days. |
| `--all` | flag | false | Disable all implicit filtering. |
| `--all-users` | flag | false | Bypass the user filter. |
| `--json` | flag | false | Emit forecast as JSON. |

---

## Recurrence

`next` supports two recurrence modes. In both cases, marking a task done with `next done` automatically creates the next instance and commits both changes in a single git commit.

### Schedule-based (`--recur-schedule`)

The next instance is determined by an RFC 5545 RRULE string. The `anchor` date (the first `start` or `due` date, or today if neither is set) pins the series so that multi-interval rules stay aligned over time.

**Supported RRULE fields**

| Field | Example | Notes |
|-------|---------|-------|
| `FREQ` | `FREQ=WEEKLY` | Required. `DAILY`, `WEEKLY`, `MONTHLY`, `YEARLY`. |
| `INTERVAL` | `INTERVAL=3` | Every Nth period. Default 1. |
| `BYDAY` | `BYDAY=MO,TU,WE,TH,FR` | Comma-separated weekday codes (`MO TU WE TH FR SA SU`). When omitted in a weekly rule, defaults to the anchor's weekday. Positional prefixes like `1MO` (first Monday of month) are **not** supported and will be rejected. |
| `BYMONTHDAY` | `BYMONTHDAY=1` | Day of month. Used with `FREQ=MONTHLY`. |

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

# Every year on Jan 1
--recur-schedule "FREQ=YEARLY;BYMONTHDAY=1"
```

When a task has both `start` and `due` dates, the start-to-due offset is preserved on every new instance. For example, a task with start=June 1, due=June 3 will next appear as start=July 1, due=July 3.

If you complete a task late (past its due date), the next occurrence is computed from `today` rather than from the original due date, so the series never schedules a date that has already passed.

### Completion-based (`--recur-completion <days>`)

The next instance is created `<days>` after the completion date (i.e., relative to when you mark it done, not a fixed calendar date).

```sh
next add "Water plants" --recur-completion 7   # once a week, whenever done
next add "Dentist check" --recur-completion 180 # every ~6 months
```

### Snap values (`--recur-snap`)

Advances the computed next date to the nearest qualifying day. Use when you want to round to a convenient boundary.

| Value | Meaning |
|-------|---------|
| `monday` … `sunday` | Advance to that weekday (keep the day if already there). |
| `next-workday` | Advance to the next Mon–Fri. |
| `dom:N` | Advance to day N of the current or next month (N = 1–28). |

```sh
# Completion-based, snapped to Saturday
next add "Weekly chore" --recur-completion 7 --recur-snap saturday

# Monthly on the 1st, snapped to next workday if the 1st is a weekend
next add "Monthly report" --recur-schedule "FREQ=MONTHLY;BYMONTHDAY=1" --recur-snap next-workday
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

All instances of the same recurring task share a `recurrence_id` UUID equal to the first instance's `id`. You can use this to query all instances of a series.

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

All list commands (`list`, `next`, `forecast`) accept a filter expression. The
trailing arguments are joined with a space and parsed as one query, so
`next list +@work -bug` and `next list '+@work -bug'` mean the same thing —
only phrases, parentheses and `#resource` tags need shell quoting.

> **A bare word searches; it is not a tag.** `next list bug` looks for "bug" in
> the title, description, notes and url. To select tasks *tagged* `bug`, write
> `next list +bug`. Every sigil form is unchanged.

### Token reference

| Token | Example | Meaning |
|-------|---------|---------|
| `<word>` | `bug`, `"cold tier"`, `arch*` | Full-text search over title, description, notes and url. A quoted phrase matches consecutive words in order; a trailing `*` matches by prefix. Matching is by whole word, so `arch` does not match "archive". |
| `<field>:<word>` | `title:rebuild`, `notes:sqlite` | The same search, restricted to one text field. |
| `+<tag>` | `+python`, `+@home`, `+#printer` | Task must have this tag (or one nested under it: `+@work` matches `@work/backend`). |
| `-<tag>` | `-@work`, `-reading` | Task must not have this tag. |
| `<field>:<value>` | `status:open`, `priority:high`, `assignee:alice`, `slug:water-plants`, `data.estimate:3` | Field equality. A comma-separated list is a set: `status:open,started`. `slug:` is exact — it does not match by prefix. |
| `<field><op><value>` | `due<+7d`, `priority>=medium`, `created>2026-08-01` | Ordered comparison (`<`, `<=`, `>`, `>=`) over `priority`, the dates, and numeric `data.*`. |
| `<field>:<low>..<high>` | `due:2026-08-01..eom` | Inclusive range. |
| `has:<field>` / `no:<field>` | `has:due`, `no:assignee`, `has:context` | Whether the field is set. `has:context` asks whether the task carries any `@` tag. |
| `is:<name>` | `is:overdue`, `is:blocked`, `is:project`, `is:recurring`, `is:closed`, `is:assigned` | Named predicates. |
| `and` `or` `not` `( )` | `+@work and (due<+7d or is:overdue)` | Booleans, also spelled `&`, `|`, `!`. Adjacency means `and`, and precedence runs `not` > `and` > `or`. Quote an operator word (`"or"`) to search for it literally. |
| `parent:<slug>` | `parent:work`, `parent:launch-blog` | Task is a descendant (direct or transitive child) of the task with this slug. |
| `context:<@tag>` | `context:@home` | Include exactly this tag for this query only, ignoring whatever the stored state includes. Exclusions still apply. |
| `user:<name>` | `user:alice` | Override the global user filter for this query only. |
| `--future` | | Include tasks with a future `start` date. |
| `--all` | | Disable all implicit filtering. |
| `--all-users` | | Bypass the user filter only. |

### Implicit filtering (default behaviour)

Unless `--all` is passed, the following tasks are always excluded:

- Tasks with `status` other than `open` or `started`
- Tasks whose `start` date is in the future
- Tasks that are blocked (any open `blocked_by` entry, or the parent of any open subtask)
- Tasks whose `assignee` does not match the active user set (tasks with no `assignee` are always shown)

The **tag state** is applied separately and is *not* disabled by `--all`, which widens
the statuses shown rather than the tags: a tag excluded on purpose stays excluded until
it is un-excluded. Tasks carrying an excluded tag are hidden, and while any tag is
included, tasks that carry no included tag are hidden too.

### Dates in a filter

An unquoted date value takes the compact forms: ISO (`2026-08-10`), a signed
offset (`+7d`, `-2w`, `+3m`, `-1y`), a named day (`today`, `tomorrow`,
`yesterday`) or an end-of-period (`eow`, `eom`, `eoy`). Quote the value to use
natural language: `due:"next monday"`.

A field that is unset never compares true — a task with no deadline is not
matched by `due<+7d` *or* by `due>+7d`.

### Not yet supported

- `score` cannot be filtered on: a task's score is computed after filtering.
- `next list --archived` accepts only `+tag` and `-tag`; a richer query is
  refused rather than silently ignored.

---

## Design Notes

### Task ID references

Task IDs are UUID v4 values. On the command line, any unambiguous prefix of at least 4
hex characters is accepted. Slugs are also accepted wherever an ID is expected.

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
