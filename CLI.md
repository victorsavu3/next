# `next` CLI Reference

A task manager with automatic urgency scoring. The binary is called `next`.

---

## Command Summary

| Command | Short description |
|---------|------------------|
| `next init` | Initialise a task repository in the current directory |
| `next add` | Add a new task |
| `next list` | List tasks sorted by urgency score |
| `next next` | Show the top N highest-scored tasks |
| `next show` | Show full details of a single task |
| `next done` | Mark a task complete (triggers recurrence if applicable) |
| `next cancel` | Mark a task cancelled |
| `next edit` | Edit fields on an existing task |
| `next delete` | Permanently delete a task (with confirmation) |
| `next move` | Change the parent of a task |
| `next open` | Open the task's URL in the default browser |
| `next project list` | List all project tasks in a tree view |
| `next project add` | Create a new project task |
| `next project show` | Show a project and all its descendants |
| `next data set` | Set a key in the task's data map |
| `next data unset` | Remove a key from the task's data map |
| `next data get` | Print the value of one key from the task's data map |
| `next context` | Show active contexts |
| `next context set` | Set the global active context filter |
| `next context clear` | Clear all active contexts |
| `next resource` | List resources and their availability |
| `next resource set` | Toggle a resource available or unavailable |
| `next forecast` | Show upcoming recurrence dates |
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

### `next init`

Initialise a new task repository in the current directory. Safe to run on an existing repository.

**Usage**

```
next init
```

What it does:

1. Runs `git init` (skipped if `.git` already exists)
2. Creates `tasks/` directory
3. Appends `.next.db` to `.gitignore` (or creates `.gitignore`; never duplicates the entry)
4. Creates an initial git commit when git user config is available

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
| `--recur-schedule <rule>` | string | none | Creates a schedule-based recurring task. Accepted rules: "every Monday", "every weekday", "1st of every month", "every 2 weeks", etc. |
| `--recur-completion <days>` | positive integer | none | Creates a completion-based recurring task. Next instance is created `<days>` after the completion date. |
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

# Subtask under an existing task
next add "Write unit tests" --parent work-backend

# Recurring task: water plants 7 days after last watering
next add "Water plants" --slug water-plants --recur-completion 7 --tag @home

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
| `--all-users` | flag | false | Bypass the user filter; show tasks for all assignees. |
| `--json` | flag | false | Emit task list as JSON. |

Filter tokens (see [Filter Syntax](#filter-syntax)) may be placed anywhere in the argument list.

**Examples**

```sh
# Default view (respects active contexts and resources)
next list

# All Python-tagged tasks
next list +python

# Tasks available at home, including those not yet started
next list context:@home --future

# Everything, bypassing all implicit filters
next list --all
```

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
# Show top 10 tasks (default)
next next

# Top 5 tasks in the work context
next next 5 context:@work

# Top 10 tasks with JSON output
next next --json
```

---

### `next show`

Display the full details of a single task, including its description, URL, data, notes, all tags, subtask list, blockers, and recurrence configuration.

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

**Examples**

```sh
next show water-plants
next show a1b2
next show a1b2 --json
```

---

### `next done`

Mark a task as complete. If the task has a recurrence rule, the next instance is created automatically. If the task has a linked Forgejo issue, the issue is closed via the API.

**Usage**

```
next done <id>
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit the completed task (and the new recurrence instance, if any) as JSON. |

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
| `--json` | flag | false | Emit the updated task as JSON. |

Trailing `+tag` and `-tag` tokens may also be used to add or remove tags without `--tag` / `--remove-tag`:

```sh
next edit water-plants +@garden -urgent
```

**Examples**

```sh
# Move a task's deadline forward
next edit a1b2 --due "next Friday"

# Change priority and add a tag
next edit a1b2 --priority high --tag @work

# Remove a tag and clear the due date
next edit a1b2 --remove-tag @home --clear-due

# Set a URL and description
next edit a1b2 --url "https://example.com/ticket-42" --description "See comments in ticket"
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
# Make a task a subtask of another
next move a1b2 --parent work-infra

# Remove the parent relationship (promote to top-level)
next move a1b2 --parent none
```

---

### `next open`

Open the URL associated with a task in the default browser (uses `xdg-open` on Linux, `open` on macOS). Fails with an error if the task has no `url` field.

**Usage**

```
next open <id>
```

**Examples**

```sh
next open a1b2
next open my-ticket
```

---

### `next project list`

List all open tasks tagged `"project"` in a tree view with their open subtask count.

**Usage**

```
next project list
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit the project list as JSON. |

---

### `next project add`

Create a new project task (shorthand for `next add --tag project`).

**Usage**

```
next project add <title> [options]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--slug <slug>` | string | none | Stable short identifier for the project. |
| `--priority <level>` | `low\|medium\|high` | `medium` | Project priority; affects child task scoring. |
| `--notes <text>` | string | none | Free-text description of the project. |
| `--parent <id>` | task ID | none | Make this project a sub-project. |
| `--json` | flag | false | Emit the new project task as JSON. |

**Examples**

```sh
next project add "Launch blog" --slug launch-blog --priority high
next project add "Work infrastructure" --slug work-infra --notes "Server and CI work"
```

---

### `next project show`

Show a project task and all its descendants in a tree view.

**Usage**

```
next project show <id>
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit the tree as JSON. |

---

### `next data set`

Set a key in the task's `data` map. Values are interpreted as JSON: numbers and booleans
are stored as their native types; anything else is stored as a string.

**Usage**

```
next data set <id> <key> <value>
```

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

### `next context`

Show the currently active context filters.

**Usage**

```
next context
```

---

### `next context set`

Replace the global active context set. All subsequent commands filter tasks by these contexts until changed or cleared.

**Usage**

```
next context set <@tag>...
```

**Examples**

```sh
next context set @home
next context set @home @errands
```

---

### `next context clear`

Clear all active contexts. After this, tasks are shown regardless of their `@` tags.

**Usage**

```
next context clear
```

---

### `next resource`

List all known resources and their current availability status.

**Usage**

```
next resource
```

---

### `next resource set`

Toggle a resource available or unavailable. Tasks tagged with an unavailable resource are hidden from the default list.

**Usage**

```
next resource set <#resource> <on|off>
```

**Examples**

```sh
next resource set #printer off   # printer is broken — hide printer tasks
next resource set #printer on    # printer repaired
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
| `--future` | flag | false | Include tasks with a future `start` date. |
| `--all` | flag | false | Disable all implicit filtering. |
| `--all-users` | flag | false | Bypass the user filter. |
| `--json` | flag | false | Emit forecast as JSON. |

**Examples**

```sh
next forecast
next forecast --days 180
next forecast +#printer
```

---

### `next sync`

Synchronise with the remote git repository: pull, rebuild the local cache, then push local commits. Stops with a detailed error message if a merge conflict is detected.

**Usage**

```
next sync
```

---

### `next import forgejo`

Import issues from a Forgejo repository as tasks. On the first run, creates one task per issue. On subsequent runs, updates only the `status` of previously imported tasks; user-edited fields are never overwritten.

**Usage**

```
next import forgejo <owner/repo> [options]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--tag <tag>` | string | none | Add an extra tag to all imported tasks. Repeatable. |
| `--json` | flag | false | Emit a summary of created and updated tasks as JSON. |

---

### `next import ical`

Import VTODO entries from an iCalendar (`.ics`) file or a webcal URL. Only the `STATUS` field is imported. Matching is done by `UID`.

**Usage**

```
next import ical <file-or-url>
```

---

### `next export ical`

Export matching tasks as a valid iCalendar file. Accepts the same filter tokens as `next list`. Output goes to stdout by default.

**Usage**

```
next export ical [filters...] [--output <file>]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--output <file>` | file path | stdout | Write output to this file instead of stdout. |
| `--future` | flag | false | Include tasks with a future `start` date. |
| `--all` | flag | false | Disable all implicit filtering. |

---

## Filter Syntax

All list commands (`list`, `next`, `forecast`, `export ical`) accept filter tokens that can be combined freely in any order. Multiple tokens of the same type are combined with AND.

### Token reference

| Token | Example | Meaning |
|-------|---------|---------|
| `+<tag>` | `+python`, `+@home`, `+#printer` | Task must have this tag. Multiple `+` tokens are ANDed. |
| `-<tag>` | `-@work`, `-reading` | Task must not have this tag. Multiple `-` tokens are ANDed. |
| `project:<path>` | `project:work`, `project:work-infra` | Task belongs to this project or any descendant. |
| `context:<@tag>` | `context:@home` | Override the global active context for this query only. |
| `user:<name>` | `user:alice` | Override the global user filter for this query only. |
| `--future` | | Include tasks with a future `start` date. |
| `--all` | | Disable all implicit filtering: context, resource, user, blocked, future start. |
| `--all-users` | | Bypass the user filter only; context and resource filters remain active. |

### Implicit filtering (default behaviour)

Unless `--all` is passed, the following tasks are always excluded from results:

- Tasks with `status` other than `open`
- Tasks whose `start` date is in the future
- Tasks that are blocked (any open `blocked_by` entry, or the parent of any open subtask)
- Tasks carrying a `#resource` tag where that resource is currently unavailable
- Tasks whose `@context` tags do not match the active context set (tasks with no `@` tags are always shown)
- Tasks whose `assignee` does not match the active user set (tasks with no `assignee` are always shown)

### Examples

```sh
# Python-tagged tasks
next list +python

# Tasks available at home, including future and not-yet-started
next list context:@home --future

# Everything, no filters
next list --all

# Upcoming recurrences that require the printer
next forecast +#printer
```

---

## Design Notes

### Binary name: `next`

The binary is named `next`, reflecting the tool's primary purpose: surfacing what to work
on next. The sub-command that lists the top N tasks is also called `next`, making
`next next` a natural invocation.

### Task ID references

Task IDs are UUID v4 values. On the command line, any unambiguous prefix of at least 4
hex characters is accepted. If the prefix matches more than one task, the command fails
with an error listing the ambiguous matches. Slugs are also accepted wherever an ID is
expected.

### Projects

A project is any task tagged `"project"`. There is no separate project entity. The
`next project add` command is a shorthand for `next add --tag project`. Child tasks
attach via `--parent`. A parent task is hidden from the default list while any of its
direct children are still open.

The project-priority factor in scoring means that tasks parented to a high-priority
project get a small score boost (+0.5), and tasks parented to a low-priority project get
a small penalty (−0.5).

### `data` field

The `data` map stores arbitrary key-value pairs. Values can be strings, numbers, or
booleans — null is not allowed. This field is intended for AI-provided metadata and tool
integrations rather than user-entered data.

### `next move` vs `next edit --parent`

`next move` is a focused command for changing a task's parent. It accepts only `--parent`
(and `--json`). `next edit` can accomplish the same thing but is more verbose.

### Why `done`, `cancel`, and `delete` are separate commands

- `done` — marks complete; triggers recurrence; may call an external API (Forgejo).
- `cancel` — marks abandoned; releases downstream blockers; no recurrence.
- `delete` — removes the TOML file from the repository permanently.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | User or input error (bad arguments, ambiguous ID, etc.) |
| `2` | System error (git conflict, network failure, DB error, etc.) |
