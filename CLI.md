# `next` CLI Design Proposal

A GTD-style task manager with automatic urgency scoring. The binary is called `next`.

---

## Command Summary

| Command | Short description |
|---------|------------------|
| `next add` | Add a task to the inbox |
| `next list` | List tasks sorted by urgency score |
| `next next` | Show the top N highest-scored tasks |
| `next show` | Show full details of a single task |
| `next done` | Mark a task complete (triggers recurrence if applicable) |
| `next cancel` | Mark a task cancelled |
| `next edit` | Edit fields on an existing task |
| `next delete` | Permanently delete a task (with confirmation) |
| `next move` | Relocate a task to a different project or stage |
| `next project list` | List all projects in a tree view |
| `next project add` | Create a new project |
| `next project show` | Show project metadata and its open tasks |
| `next context` | Show active contexts |
| `next context set` | Set the global active context filter |
| `next context clear` | Clear all active contexts |
| `next resource` | List resources and their availability |
| `next resource set` | Toggle a resource available or unavailable |
| `next forecast` | Show upcoming recurrence dates |
| `next review` | Interactive GTD weekly review walkthrough |
| `next sync` | Pull from git remote, rebuild cache, push |
| `next user` | Show active user filter |
| `next user set` | Set the global active user filter |
| `next user clear` | Clear the user filter |
| `next user list` | List all assignees across all tasks |
| `next import forgejo` | Import issues from a Forgejo repository |
| `next import ical` | Import VTODO entries from an iCalendar file or URL |
| `next export ical` | Export tasks as an iCalendar VTODO file |

---

## Command Reference

### `next add`

Add a new task. Tasks land in the inbox stage by default.

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
| `--project <path>` | project path | none | Assign to a project. Use `/` for nesting, e.g. `work/infra`. |
| `--tag <tag>` | string | none | Add a tag. Repeatable. Use `@` prefix for context tags, `$` prefix for resource tags. |
| `--parent <id>` | task ID | none | Makes this task a subtask of the given task. The parent is blocked until all children complete. |
| `--blocked-by <id>` | task ID | none | Declares an explicit blocker. Repeatable. |
| `--notes <text>` | string | none | Multi-line free-text notes. |
| `--stage <stage>` | `inbox\|project\|waiting\|someday` | `inbox` | GTD stage to place the task in. |
| `--wait-for <who>` | string | none | Sets `waiting_for` free-text and moves the task to `waiting` stage. |
| `--recur schedule <rule>` | string | none | Creates a schedule-based recurring task. Accepted rules: "every Monday", "every weekday", "1st of every month", "every 2 weeks", etc. |
| `--recur completion <days>` | positive integer | none | Creates a completion-based recurring task. Next instance is created `<days>` after the completion date. |
| `--long-term` | flag | false | Disables the age factor from scoring. Suitable for background or low-priority ideas. |
| `--adjust <value>` | float | 0.0 | Manual score adjustment added directly to the computed urgency score. Positive boosts, negative penalises. |
| `--assignee <name>` | string | none | Assign the task to a user. Used by the user filter. |
| `--json` | flag | false | Emit the created task as JSON on stdout. |

**Examples**

```sh
# Minimal inbox capture
next add "Call dentist"

# Task with deadline and context tag
next add "Submit tax forms" --due "April 15" --tag @home --priority high

# Subtask under an existing task
next add "Write unit tests" --parent a1b2c3d4 --project work/backend

# Recurring task: water plants 7 days after last watering
next add "Water plants" --recur completion 7 --tag @home

# Scheduled recurring task with a start date
next add "Monthly budget review" --recur schedule "1st of every month" --due "June 1"
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
| `--all-users` | flag | false | Bypass the user filter; show tasks for all assignees. |
| `--stage <stage>` | `inbox\|project\|waiting\|someday` | none | Restrict output to one GTD stage. |
| `--json` | flag | false | Emit task list as JSON. |

Filter tokens (see [Filter Syntax](#filter-syntax) section) may be placed anywhere in the argument list.

**Examples**

```sh
# Default view (respects active contexts and resources)
next list

# All Python-tagged tasks in the work project tree
next list +python project:work

# Everything in someday/maybe, bypassing all implicit filters
next list --all --stage someday

# Tasks available at home, including those not yet started
next list context:@home --future
```

---

### `next next`

Show the top N tasks by urgency score. Identical filtering rules to `next list`, but limited to a fixed number of results. This is the primary "what should I do now?" command.

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
# Show top 10 tasks (default)
next next

# Top 5 tasks in the work project
next next 5 project:work

# Top 3 tasks available at home right now
next next 3 context:@home

# Top 10 tasks with JSON output (for piping)
next next --json
```

---

### `next show`

Display the full details of a single task, including its notes, all tags, subtask list, blockers, recurrence configuration, and external references.

**Usage**

```
next show <id>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID or any unambiguous prefix (minimum 4 hex characters). |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit full task details as JSON. |

**Examples**

```sh
# Show task by full ID
next show a1b2c3d4-e5f6-7890-abcd-ef1234567890

# Show task by short prefix
next show a1b2

# Show task as JSON for scripting
next show a1b2 --json
```

---

### `next done`

Mark a task as complete. If the task has a recurrence rule, the next instance is created automatically. If the task has a linked Forgejo issue, the issue is closed via the API.

**Usage**

```
next done <id>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID or any unambiguous prefix. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit the completed task (and the new recurrence instance, if any) as JSON. |

**Examples**

```sh
next done a1b2c3d4

# Done with JSON output (shows new recurrence instance if created)
next done a1b2 --json
```

---

### `next cancel`

Mark a task as cancelled. Cancelled tasks are removed from the default view and are no longer considered blockers.

**Usage**

```
next cancel <id>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID or any unambiguous prefix. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit the updated task as JSON. |

**Examples**

```sh
next cancel a1b2c3d4

next cancel a1b2 --json
```

---

### `next edit`

Modify one or more fields on an existing task. Only the flags you supply are changed; all other fields are left untouched. To clear an optional field, pass an empty string or the special value `none` where applicable.

**Usage**

```
next edit <id> [options]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID or any unambiguous prefix. |

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
| `--assignee <name>` | string | — | Assign the task to a user. |
| `--clear-assignee` | flag | false | Remove the assignee. |
| `--json` | flag | false | Emit the updated task as JSON. |

**Examples**

```sh
# Move a task's deadline forward
next edit a1b2 --due "next Friday"

# Change priority and add a tag
next edit a1b2 --priority high --tag @work

# Remove a tag and clear the due date
next edit a1b2 --remove-tag @home --clear-due

# Update notes on a task
next edit a1b2 --notes "Waiting for invoice number from finance."
```

---

### `next delete`

Permanently remove a task from the repository. Prompts for confirmation unless `--yes` is passed. This operation cannot be undone (though git history preserves the file).

**Usage**

```
next delete <id>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID or any unambiguous prefix. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--yes` | flag | false | Skip confirmation prompt. |

**Examples**

```sh
next delete a1b2c3d4

# Non-interactive deletion (for scripting)
next delete a1b2 --yes
```

---

### `next move`

Relocate a task to a different project or GTD stage without changing any other fields. At least one of `--project` or `--stage` must be supplied.

**Usage**

```
next move <id> [--project <path>] [--stage <stage>]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<id>` | task ID | — | Full UUID or any unambiguous prefix. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--project <path>` | project path | — | Destination project. Use `/` for nesting. Pass `none` to remove project membership. |
| `--stage <stage>` | `inbox\|project\|waiting\|someday` | — | Destination GTD stage. |
| `--json` | flag | false | Emit the updated task as JSON. |

**Examples**

```sh
# Move from inbox into a project
next move a1b2 --project work/infra --stage project

# Move to someday/maybe
next move a1b2 --stage someday

# Remove project membership and send to inbox
next move a1b2 --project none --stage inbox
```

---

### `next project list`

List all projects in a tree view with their priority and description.

**Usage**

```
next project list
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit the project tree as JSON. |

**Examples**

```sh
next project list

next project list --json
```

---

### `next project add`

Create a new project. The path uses `/` as a separator for nesting. Parent projects do not need to be created first — intermediary directories are created automatically.

**Usage**

```
next project add <path> [options]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<path>` | project path | — | Slash-separated path, e.g. `work` or `work/infra`. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--priority <level>` | `low\|medium\|high` | `medium` | Project priority, used as an offset in task scoring. |
| `--description <text>` | string | none | Optional description shown in `project show`. |
| `--json` | flag | false | Emit the new project as JSON. |

**Examples**

```sh
next project add work/infra --priority high --description "Server and CI infrastructure"

next project add personal/reading
```

---

### `next project show`

Show metadata and open tasks for a project, including tasks in all sub-projects.

**Usage**

```
next project show <path>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<path>` | project path | — | Exact project path. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit project details and tasks as JSON. |

**Examples**

```sh
next project show work/infra

next project show work --json
```

---

### `next context`

Show the currently active context filters.

**Usage**

```
next context
```

**Examples**

```sh
next context
# Active contexts: @home
```

---

### `next context set`

Replace the global active context set. All subsequent commands filter tasks by these contexts until changed or cleared.

**Usage**

```
next context set <@tag>...
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<@tag>...` | one or more `@` tags | — | The new active context set. Replaces any previously set contexts. |

**Examples**

```sh
# Single context
next context set @home

# Multiple contexts active simultaneously
next context set @home @errands

# Switch to work context
next context set @work
```

---

### `next context clear`

Clear all active contexts. After this, tasks are shown regardless of their `@` tags.

**Usage**

```
next context clear
```

**Examples**

```sh
next context clear
```

---

### `next resource`

List all known resources and their current availability status.

**Usage**

```
next resource
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit resources as JSON. |

**Examples**

```sh
next resource
# printer:  available
# vacation: unavailable
```

---

### `next resource set`

Toggle a resource available or unavailable. Tasks tagged with an unavailable resource are hidden from the default list.

**Usage**

```
next resource set <$resource> <on|off>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<$resource>` | resource tag | — | The `$`-prefixed resource tag to configure. |
| `<on\|off>` | `on\|off` | — | `on` = available, `off` = unavailable. |

**Examples**

```sh
# Printer is broken — hide printer tasks
next resource set $printer off

# Back from holiday
next resource set $vacation off

# Printer repaired
next resource set $printer on
```

---

### `next user`

Show the currently active user filter.

**Usage**

```
next user
```

**Examples**

```sh
next user
# Active users: alice, bob
```

---

### `next user set`

Set the global active user filter. Tasks assigned to users not in this set are hidden from
the default list. Unassigned tasks are always visible.

**Usage**

```
next user set <name>...
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<name>...` | one or more usernames | — | The new active user set. Replaces any previously set users. |

**Examples**

```sh
# Focus on your own tasks
next user set alice

# Show tasks for two team members
next user set alice bob
```

---

### `next user clear`

Clear the user filter. After this, all tasks are shown regardless of their `assignee`.

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

**Examples**

```sh
next user list
# * alice
#   bob
#   carol
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
| `--future` | flag | false | Include tasks with a future `start` date. |
| `--all` | flag | false | Disable all implicit filtering. |
| `--all-users` | flag | false | Bypass the user filter. |
| `--stage <stage>` | `inbox\|project\|waiting\|someday` | none | Filter by stage. |
| `--json` | flag | false | Emit forecast as JSON. |

**Examples**

```sh
# Upcoming recurrences for the next 90 days
next forecast

# Extend horizon to 6 months
next forecast --days 180

# Only printer-requiring recurrences
next forecast +$printer

# Recurrences in the work project tree
next forecast project:work
```

---

### `next review`

Interactive GTD weekly review. Walks through all four stages in order (inbox, waiting-for, someday/maybe, projects), presenting each task and prompting for an action. Interruptible with Ctrl-C at any time; no changes are written until you confirm each action.

**Usage**

```
next review
```

At each task the tool prompts:

```
[d] done   [c] cancel   [e] edit   [m] move   [s] skip   [q] quit
```

**Examples**

```sh
next review
```

---

### `next sync`

Synchronise with the remote git repository: pull, rebuild the local cache, then push local commits. Stops with a detailed error message if a merge conflict is detected; conflicts must be resolved manually.

**Usage**

```
next sync
```

**Examples**

```sh
next sync
```

---

### `next import forgejo`

Import issues from a Forgejo repository as tasks. On the first run, creates one task per issue. On subsequent runs, updates only the `status` of previously imported tasks; user-edited fields (title, notes, priority, tags beyond issue labels) are never overwritten.

Marking an imported task `done` via `next done` also closes the corresponding Forgejo issue via the API.

**Usage**

```
next import forgejo <owner/repo> [options]
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<owner/repo>` | string | — | Forgejo repository in `owner/repo` format. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--project <path>` | project path | none | Assign all imported tasks to this project. |
| `--tag <tag>` | string | none | Add an extra tag to all imported tasks. Repeatable. |
| `--json` | flag | false | Emit a summary of created and updated tasks as JSON. |

**Examples**

```sh
# Import all issues from a repository
next import forgejo victor/task-manager

# Import into a specific project and tag as external
next import forgejo victor/myapp --project work/myapp --tag external

# Re-import to sync status changes
next import forgejo victor/task-manager
```

---

### `next import ical`

Import VTODO entries from an iCalendar (`.ics`) file or a webcal URL. Only the `STATUS` field is imported; all other fields on existing tasks are left untouched. Matching is done by `UID`.

**Usage**

```
next import ical <file-or-url>
```

**Arguments**

| Name | Type | Default | Description |
|------|------|---------|-------------|
| `<file-or-url>` | path or URL | — | Path to a local `.ics` file, or an `http(s)://` / `webcal://` URL. |

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit a summary of created and updated tasks as JSON. |

**Examples**

```sh
# Import from a local file
next import ical ~/Downloads/tasks.ics

# Import from a webcal feed
next import ical https://calendar.example.com/tasks.ics
```

---

### `next export ical`

Export matching tasks as a valid iCalendar file with one VTODO per task. Accepts the same filter tokens as `next list`. Output goes to stdout by default.

**Usage**

```
next export ical [filters...] [--output <file>]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--output <file>` | file path | stdout | Write the iCalendar output to this file instead of stdout. |
| `--future` | flag | false | Include tasks with a future `start` date. |
| `--all` | flag | false | Disable all implicit filtering. |
| `--stage <stage>` | `inbox\|project\|waiting\|someday` | none | Filter by stage. |

**Examples**

```sh
# Export all default-visible tasks to stdout
next export ical

# Export work project tasks to a file
next export ical project:work --output work-tasks.ics

# Export everything, no filters
next export ical --all --output full-export.ics
```

---

## Filter Syntax

All list commands (`list`, `next`, `forecast`, `export ical`) accept filter tokens that can be combined freely in any order. Multiple tokens of the same type are combined with AND.

### Token reference

| Token | Example | Meaning |
|-------|---------|---------|
| `+<tag>` | `+python`, `+@home`, `+$printer` | Task must have this tag. Multiple `+` tokens are ANDed. |
| `-<tag>` | `-@work`, `-reading` | Task must not have this tag. Multiple `-` tokens are ANDed. |
| `project:<path>` | `project:work`, `project:work/infra` | Task belongs to this project or any descendant. |
| `context:<@tag>` | `context:@home` | Override the global active context for this query only. |
| `user:<name>` | `user:alice` | Override the global user filter for this query only. |
| `--future` | | Include tasks with a future `start` date and planned recurrence instances from recurring tasks. |
| `--all` | | Disable all implicit filtering: context, resource, user, blocked, future start. |
| `--all-users` | | Bypass the user filter only; context and resource filters remain active. |
| `--stage <stage>` | `--stage inbox` | Restrict to one GTD stage. |

### Implicit filtering (default behaviour)

Unless `--all` is passed, the following tasks are always excluded from results:

- Tasks with `status` other than `open`
- Tasks whose `start` date is in the future
- Tasks that are blocked (any open `blocked_by` entry, or the parent task of any open subtask)
- Tasks carrying a `$resource` tag where that resource is currently unavailable
- Tasks whose `@context` tags do not match the active context set (when a context is active; tasks with no `@` tags are always shown)
- Tasks whose `assignee` does not match the active user set (when `active_users` is non-empty; tasks with no `assignee` are always shown)

### Context override

`context:@name` overrides the globally active context for a single invocation without modifying `state.toml`. It replaces — rather than extends — the active set for that query.

### Combining tokens

Tokens of the same kind are ANDed; tokens of different kinds are also ANDed with each other:

```sh
# Tasks that are tagged both @home AND python, in any project
next list +@home +python

# Python tasks NOT in the work tree
next list +python -project:work

# This is a shell error — use quotes or --stage:
next list --stage inbox +@home
```

### Examples

```sh
# Python-tagged tasks in the work project tree
next list +python project:work

# Tasks available at home, including future and not-yet-started
next list context:@home --future

# Everything in someday/maybe with no filtering at all
next list --all --stage someday

# Upcoming recurrences that require the printer
next forecast +$printer

# Export only inbox tasks tagged @home
next export ical --stage inbox +@home --output home-inbox.ics
```

---

## Design Notes

### Binary name: `next`

The binary is named `next`, reflecting the tool's primary purpose: surfacing what to work on next. The sub-command that lists the top N tasks is also called `next`, making `next next` a natural invocation. This mirrors the Unix convention where a binary and its default sub-command share a name (e.g. `git git`).

### Task ID references

Task IDs are UUID v4 values. On the command line, any unambiguous prefix of at least 4 hex characters is accepted. If the prefix matches more than one task, the command fails with an error listing the ambiguous matches. This mirrors how `git` handles object hashes.

Subtask and blocker relationships are also specified by prefix on the command line (`--parent a1b2`, `--blocked-by a1b2`), but are stored as full UUIDs in the TOML file.

### Tag prefixes on the CLI

Tags are plain strings. The `@` and `$` prefix conventions are interpretive — there is no separate flag for "add a context tag" vs "add a freeform tag". The same `--tag` flag handles all three kinds:

```sh
next add "Fix printer" --tag @home --tag $printer --tag hardware
```

This keeps the `add` / `edit` interface simple and allows the prefix convention to be extended in future without changing the CLI surface. The special filtering behaviour of `@` and `$` tags is implicit and described in the documentation; the user writes the full tag string everywhere.

In filter tokens, the same convention applies: `+@home` targets a context tag, `+$printer` targets a resource tag, `+python` targets a freeform tag.

### Why `context set` and `resource set` are separate sub-commands, not flags on `list`

`context:@name` on a list command is a per-query override. `context set` mutates `state.toml` and persists until changed. Keeping these as distinct sub-commands makes the distinction explicit and avoids confusion between "I want to filter this one query" and "I want to switch my global working mode". The same logic applies to `resource set`.

### Why `done`, `cancel`, and `delete` are separate commands

These three operations have meaningfully different semantics:

- `done` — marks complete; triggers recurrence; may call an external API (Forgejo).
- `cancel` — marks abandoned; releases downstream blockers; no recurrence.
- `delete` — removes the TOML file from the repository permanently.

Merging them into a single `next status` flag would obscure these side effects and make the intent less readable in shell history.

### `next move` vs `next edit --project --stage`

`next move` exists as a shortcut for the very common GTD operation of processing inbox items into the right project and stage. It requires at least one of `--project` or `--stage`, making it self-documenting in shell history. `next edit` can accomplish the same thing but is verbose for this frequent workflow.

### `--all` vs `--future`

`--all` disables every implicit filter at once: useful for administration, scripting, and auditing. `--future` is a softer opt-in that adds only tasks with a future start date and planned recurrence instances — it keeps context and resource filtering active. This distinction lets users preview their upcoming schedule without flooding the list with irrelevant tasks from other contexts.

### Recurrence syntax

Schedule-based recurrence rules are stored as human-readable strings ("every Monday", "1st of every month"). This string is what the user types at `add` time and what is shown by `show` and `forecast`. The tool translates these strings to RFC 5545 RRULE internally. Unknown or unparseable strings are rejected at `add` time with a clear error, not silently stored and discovered later.

Completion-based recurrence uses a plain integer day count to avoid ambiguity with months and "business days".

### Forgejo integration and `done`

`next done` on a Forgejo-imported task closes the issue via the API automatically. This is the only write-back to Forgejo; no other mutations (edit, cancel, delete) propagate upstream. If the API call fails, the task is still marked done locally and the error is printed as a warning. The user can re-close the issue manually.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | User or input error (bad arguments, ambiguous ID, unknown project, etc.) |
| `2` | System error (git conflict, network failure, DB error, etc.) |

### `--json` output

Every command that produces output supports `--json`. The JSON schema is stable across patch releases; breaking changes increment a `schema_version` field in the output envelope. Plain-text output uses colour when stdout is a TTY and falls back to plain text when piped.
