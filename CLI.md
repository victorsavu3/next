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
| `next tree` | Show all tasks in a parent-child tree |
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
| `next tag describe` | Set a description for any tag |
| `next tag clear-description` | Remove a tag description |
| `next context` | Show active contexts |
| `next context set` | Set the global active context filter |
| `next context clear` | Clear all active contexts |
| `next context describe` | Set a description for a context tag |
| `next context clear-description` | Remove a context description |
| `next resource` | List resources and their availability |
| `next resource set` | Toggle a resource available or unavailable |
| `next resource describe` | Set a description for a resource tag |
| `next resource clear-description` | Remove a resource description |
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

# Project task (just a task with the "project" tag)
next add "Launch blog" --slug launch-blog --tag project --priority high

# Subtask under an existing project
next add "Write first post" --parent launch-blog

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
next next
next next 5 context:@work
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

---

### `next tree`

Show all tasks in a parent-child tree. Root tasks (no parent) are listed at the top;
child tasks are indented under their parent. Tasks tagged `project` that have children
are marked with `[project]`.

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

### `next done`

Mark a task as complete. If the task has a recurrence rule, the next instance is created automatically.

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

### `next tag`

List all tags that appear on any task, together with any tags that have a stored
description. Tags are grouped into three sections: Contexts (`@`), Resources (`#`), and
Freeform. Descriptions are shown inline.

**Usage**

```
next tag
```

---

### `next tag describe`

Set a human-readable description for a tag, context, or resource. The description is
stored in `state.toml` under `tag_descriptions` and is shown in `next tag`,
`next context`, and `next resource` output.

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

### `next context`

Show the currently active context filters and their descriptions (if any).

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

### `next context describe`

Set a human-readable description for a context tag. Equivalent to
`next tag describe <@tag> <description>` but validates that the tag starts with `@`.

**Usage**

```
next context describe <@tag> <description>
```

**Examples**

```sh
next context describe @home "Home tasks: kitchen, garden, errands"
next context describe @work/frontend "Frontend development at the office"
```

---

### `next context clear-description`

Remove the stored description for a context tag.

**Usage**

```
next context clear-description <@tag>
```

---

### `next resource`

List all known resources and their current availability status. Descriptions are shown
inline when set.

**Usage**

```
next resource [--json]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit resources as a JSON object keyed by resource tag. |

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

### `next resource describe`

Set a human-readable description for a resource tag. Equivalent to
`next tag describe <#tag> <description>` but validates that the tag starts with `#`.

**Usage**

```
next resource describe <#tag> <description>
```

**Examples**

```sh
next resource describe #printer "Office laser printer, 2nd floor"
next resource describe #vacation "Away from keyboard — all resource tasks hidden"
```

---

### `next resource clear-description`

Remove the stored description for a resource tag.

**Usage**

```
next resource clear-description <#tag>
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

---

### `next sync`

Synchronise with the remote git repository: pull, rebuild the local cache, then push local commits. Stops with a detailed error message if a merge conflict is detected.

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

### `next import forgejo`

Import issues from a Forgejo repository as tasks. On the first run, creates one task per open issue. On subsequent runs, updates only the `status` of previously imported tasks.

**Usage**

```
next import forgejo <owner/repo> [options]
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--parent <id>` | task ID | none | Attach all imported tasks as subtasks of this task. |
| `--tag <tag>` | string | none | Add an extra tag to all imported tasks. Repeatable. |
| `--json` | flag | false | Emit a summary of created and updated tasks as JSON. |

---

### `next import ical`

Import VTODO entries from an iCalendar (`.ics`) file or a webcal URL.

**Usage**

```
next import ical <file-or-url>
```

**Options**

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--json` | flag | false | Emit a summary of created and updated tasks as JSON. |

---

### `next export ical`

Export matching tasks as a valid iCalendar file. Output goes to stdout by default.

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

All list commands (`list`, `next`, `forecast`, `export ical`) accept filter tokens that can be combined freely in any order.

### Token reference

| Token | Example | Meaning |
|-------|---------|---------|
| `+<tag>` | `+python`, `+@home`, `+#printer` | Task must have this tag. |
| `-<tag>` | `-@work`, `-reading` | Task must not have this tag. |
| `project:<path>` | `project:work`, `project:launch-blog` | Task belongs to this project or any descendant. |
| `context:<@tag>` | `context:@home` | Override the global active context for this query only. |
| `user:<name>` | `user:alice` | Override the global user filter for this query only. |
| `--future` | | Include tasks with a future `start` date. |
| `--all` | | Disable all implicit filtering. |
| `--all-users` | | Bypass the user filter only. |

### Implicit filtering (default behaviour)

Unless `--all` is passed, the following tasks are always excluded:

- Tasks with `status` other than `open`
- Tasks whose `start` date is in the future
- Tasks that are blocked (any open `blocked_by` entry, or the parent of any open subtask)
- Tasks carrying a `#resource` tag where that resource is currently unavailable
- Tasks whose `@context` tags do not match the active context set (tasks with no `@` tags are always shown)
- Tasks whose `assignee` does not match the active user set (tasks with no `assignee` are always shown)

---

## Design Notes

### Task ID references

Task IDs are UUID v4 values. On the command line, any unambiguous prefix of at least 4
hex characters is accepted. Slugs are also accepted wherever an ID is expected.

### Projects and subtasks

A project is any task tagged `"project"`. There is no separate project entity — projects
are plain tasks. Give a task the `project` tag when you want it to act as a container.
Child tasks attach via `--parent`. A parent task is hidden from the default scored list
while any of its direct children are still open.

Use `next tree` to see all tasks in their parent-child structure. Use `next show <id>`
to inspect a single task and its direct children.

### Tag descriptions

Descriptions are stored in `state.toml` under `[tag_descriptions]` as a flat map from
full tag string to description text. All three commands (`next tag describe`,
`next context describe`, `next resource describe`) write to the same map. The key is
always the full tag including prefix (`@work`, `#printer`, `python`).

### `data` field

The `data` map stores arbitrary key-value pairs (strings, numbers, booleans — null is
not allowed). Intended for AI-provided metadata and tool integrations.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | User or input error (bad arguments, ambiguous ID, etc.) |
| `2` | System error (git conflict, network failure, DB error, etc.) |
