# Task Manager — Goals

This file captures rough goals and ideas to be refined into structured documentation later.

## What I want to build

A CLI task manager built around GTD (Getting Things Done). The tool should make it fast
to capture tasks without friction and then process them into the right stage. The primary
purpose of the tool is to surface *what to work on next* via automatic scoring, so the
user rarely has to think about ordering.

**Storage model**: each task is a separate TOML file committed to a git repository — this
is the source of truth and enables offline-first sync across machines. SQLite is a derived
cache stored in `~/.cache/task-manager/` and rebuilt lazily (by comparing mtimes / git
HEAD) before each command. Git merge conflicts from concurrent offline edits are resolved
manually by the user.

Output should be pipe-friendly (clean JSON / plain text) so the tool integrates naturally
with Claude Code and other CLI tools rather than embedding AI directly.

## GTD stages

- **Inbox** — default landing zone; tasks land here on capture, no metadata required
- **Projects** — a GTD marker (`stage = project`) for active committed outcomes; any task
  can have subtasks regardless of stage, and any task can serve as a parent.
- **Waiting-for** — blocked on someone else; stores who as a free-text string
- **Someday/Maybe** — low-commitment ideas to revisit later

## Key features

### Capture & organisation
- Add a task to inbox in one command with minimal typing
- Move tasks from inbox to the right stage / project
- Due dates — optional deadline on any task
- Priority — high / medium / low urgency on each task
- Tags — see unified tag system below
- Projects are plain tasks in the `project` stage; hierarchy is expressed through
  `parent_id`, not a separate project concept
- Tasks may have a user-provided **slug** (e.g. `"water-plants"`, `"work-infra"`) as a
  stable short identifier, used to reference the task as a parent or blocker without
  knowing its UUID

### Unified tag system
All labels on a task are tags. Prefix conventions give some tags special meaning:

| Prefix | Kind | Example | Implicit filtering behaviour |
|--------|------|---------|------------------------------|
| `@` | Context | `@home`, `@work` | Only tasks matching an active context (or with no `@` tag) are shown |
| `$` | Resource | `$printer`, `$vacation` | Tasks requiring an unavailable resource are hidden |
| *(none)* | Freeform | `python`, `reading` | No implicit effect; used for manual queries |

**Active contexts** are set globally (e.g. `next context @home`). When one or more contexts
are active, tasks with no `@` tag are always shown; tasks with at least one `@` tag are
shown only if they share a tag with the active set.

**Resource availability** is set globally (e.g. `next resource $printer off`). Tasks
carrying a `$resource` tag whose resource is marked unavailable are hidden from the
default list and excluded from scoring.

### Nested tasks (subtasks)
Any task can have subtasks to any depth via `parent_id` — there is no special project
type. A parent task is hidden from the default scored list until all direct children are
resolved, but the user can mark it done explicitly at any time. Long-running parent tasks
should set `long_term = true` to avoid age-based scoring pressure while in progress; all
tasks are completable eventually.

### Recurrence
Two distinct recurrence models:

- **Schedule-based** — task recurs on a fixed calendar rule (e.g. "1st of every month",
  "every Monday"). At most one future instance is visible at a time; it appears in the
  list as soon as the previous instance is completed (or when the tool first runs for that
  period). A `next forecast` command shows the list of upcoming due dates for review.
- **Completion-based** — task recurs a fixed interval after the last completion (e.g.
  "water plants 7 days after last watered"). The next instance is created when the current
  one is marked done, with the due date calculated from the completion timestamp.

### Blocking tasks
A task can declare that it is blocked by one or more other tasks. Blocked tasks are
excluded from scoring and the default view until all blockers are resolved.

### Automatic scoring
Urgency score computed per task (Taskwarrior-style) from:

- **Due date / urgency** — overdue and near-due tasks score higher
- **Priority level** — high/medium/low weight applied to the base score
- **Project priority** — parent project's priority multiplies or offsets the score
- **Age** — older tasks float up, unless the task has `long_term = true` or a future
  `start` date, in which case the age factor is zeroed out
- **User adjustment** — a manual numeric boost or penalty the user can apply to any task

The default `next` / `list` command ranks tasks by score descending, after filtering out
blocked tasks, tasks with unavailable `$` tags, and tasks not matching the active `@`
contexts. Tasks with a `start` date in the future are hidden entirely until that date.

### Query interface
All list commands accept filters that can be combined freely:

| Filter | Example | Meaning |
|--------|---------|---------|
| `+tag` | `+@home`, `+python` | Task must have this tag |
| `-tag` | `-@work` | Task must not have this tag |
| `project:<path>` | `project:work/infra` | Task is in this project or any sub-project |
| `context:<name>` | `context:@home` | Override active context for this query |
| `--future` | | Include tasks with a future `start` date and planned recurrence instances |
| `--all` | | Disable all implicit filtering (contexts, resources, blocked, start date) |
| `--stage <stage>` | `--stage inbox` | Filter by GTD stage |

Examples:
```
next list +python project:work          # python-tagged tasks in the work project tree
next list context:@home --future        # upcoming tasks available at home
next list --all --stage someday         # everything in someday/maybe, unfiltered
next forecast +$printer                 # upcoming recurrences that need the printer
```

### Reminders
Overdue and due-today tasks are surfaced prominently at the top of every `list` /
`next` run — no daemon or background process required.

### Weekly review
An interactive `next review` command walks through each GTD stage in turn, prompting the
user to process inbox items, check waiting-for tasks, and triage someday/maybe.

### Sync
- All task data lives as one TOML file per task in a git repository
- SQLite DB is a local cache in `~/.cache/task-manager/`, rebuilt lazily before each command
- `next sync` pulls from the remote git repo, rebuilds the DB, then pushes local commits
- Offline edits accumulate as local git commits; sync merges them when connectivity returns
- Git merge conflicts (two machines editing the same task file) are resolved manually

### Forgejo integration
- **Import** — pull issues from a Forgejo repository in as tasks, preserving title, body,
  labels (mapped to freeform tags), and open/closed state
- **Completion sync** — marking an imported task complete closes the corresponding Forgejo
  issue; no other fields are written back
- Each imported task stores its Forgejo issue URL so duplicates are avoided on re-import

### WebCal (iCalendar) support
- **Import** — read a `.ics` / webcal feed and create or update tasks based on VTODO
  entries; only task status is imported (NEEDS-ACTION → open, COMPLETED / CANCELLED →
  done); all other fields are ignored
- **Export** — write tasks as VTODO entries; maps title, due date, priority, and
  completion status; fields with no iCalendar equivalent are omitted gracefully

### Pipe-friendly output
Every list/show command supports a `--json` flag. Plain-text output is structured and
stable enough to pipe to Claude Code or other tools.

## Out of scope (for now)

- Built-in AI; AI is invoked externally via piped output
- GUI or TUI
- Multi-user / shared task lists
- OS desktop notifications or background reminder daemon

## Open questions

- **Recurrence — lead time** — for schedule-based tasks, should the next instance become
  visible immediately after the previous one completes, or only when today reaches the due
  date? (Current assumption: visible immediately, scored by due date proximity.)
- **Subtask files** — should subtasks be embedded in the parent's TOML file or stored as
  separate files with a `parent` reference? Separate files are better for git diffs;
  embedded is simpler to edit by hand.
- **Context matching with no active context set** — when no `@` context is active, should
  all tasks be shown regardless of their `@` tags, or only tasks with no `@` tags?
