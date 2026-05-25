# Task Manager — Goals

This file captures rough goals and ideas to be refined into structured documentation later.

## What I want to build

A CLI task manager that surfaces *what to work on next* via automatic urgency scoring.
The tool integrates naturally with AI assistants: pipe-friendly JSON output lets external
tools (Claude Code, scripts) read and annotate tasks without embedding AI directly.

**Storage model**: each task is a separate TOML file committed to a git repository — this
is the source of truth and enables offline-first sync across machines. SQLite is a derived
cache stored at `.next.db` and rebuilt lazily (by comparing git HEAD) before each command.
Git merge conflicts from concurrent offline edits are resolved manually by the user.

## Philosophy

The tool should tell you **not** to do, rather than ask you to organise. The scoring
engine surfaces the most urgent work automatically; the user should rarely need to think
about ordering. There is no review step — if you capture a task with good metadata, the
scorer handles the rest.

AI is the primary path for enriching task metadata (`description`, `url`, `data`). The
tool provides a clean pipe-friendly interface; AI reads the list, annotates tasks, and
sets scores or data fields externally.

## Key features

### Capture & organisation
- Add a task in one command with minimal typing
- Due dates — optional deadline on any task
- Priority — high / medium / low urgency on each task
- Tags — see unified tag system below
- Projects are plain tasks with a `"project"` tag; hierarchy is expressed through
  `parent_id`, not a separate project type
- Tasks may have a user-provided **slug** (e.g. `"water-plants"`, `"work-infra"`) as a
  stable short identifier for referencing as parent or blocker
- `description` — brief context summary for a task
- `url` — a link to a ticket, doc, or reference (`next open <id>` to launch)
- `data` — arbitrary JSON key-value map for AI-provided metadata and tool integrations

### Unified tag system
All labels on a task are tags. Prefix conventions give some tags special meaning:

| Prefix | Kind | Example | Implicit filtering behaviour |
|--------|------|---------|------------------------------|
| `@` | Context | `@home`, `@work` | Only tasks matching an active context (or with no `@` tag) are shown |
| `#` | Resource | `#printer`, `#vacation` | Tasks requiring an unavailable resource are hidden |
| *(none)* | Freeform | `python`, `project` | No implicit effect; used for manual queries |

**Active contexts** are set globally (e.g. `next context set @home`). When one or more
contexts are active, tasks with no `@` tag are always shown; tasks with at least one `@`
tag are shown only if they share a tag with the active set.

**Resource availability** is set globally (e.g. `next resource set #printer off`). Tasks
carrying a `#resource` tag whose resource is marked unavailable are hidden from the
default list and excluded from scoring.

### Nested tasks (subtasks)
Any task can have subtasks to any depth via `parent_id` — there is no special project
type. A parent task is hidden from the default scored list until all direct children are
resolved, but the user can mark it done explicitly at any time. Long-running parent tasks
should set `long_term = true` to avoid age-based scoring pressure while in progress.

### Recurrence
Two distinct recurrence models:

- **Schedule-based** — task recurs on a fixed calendar rule (e.g. "1st of every month",
  "every Monday"). At most one future instance is visible at a time.
- **Completion-based** — task recurs a fixed interval after the last completion (e.g.
  "water plants 7 days after last watered"). The next instance is created when the current
  one is marked done.

### Blocking tasks
A task can declare that it is blocked by one or more other tasks. Blocked tasks are
excluded from scoring and the default view until all blockers are resolved.

### Automatic scoring
Urgency score computed per task from:

- **Due date / urgency** — overdue and near-due tasks score higher
- **Priority level** — high/medium/low weight applied to the base score
- **Project (parent) priority** — parent task's priority offsets the score (+0.5 high, −0.5 low)
- **Age** — older tasks float up, unless the task has `long_term = true` or a future
  `start` date, in which case the age factor is zeroed out
- **User adjustment** — a manual numeric boost or penalty the user can apply

The default `next` / `list` command ranks tasks by score descending, after filtering out
blocked tasks, tasks with unavailable `#` tags, and tasks not matching the active `@`
contexts. Tasks with a `start` date in the future are hidden entirely until that date.

### Query interface
All list commands accept filters that can be combined freely:

| Filter | Example | Meaning |
|--------|---------|---------|
| `+tag` | `+@home`, `+python` | Task must have this tag |
| `-tag` | `-@work` | Task must not have this tag |
| `project:<slug>` | `project:work-infra` | Task is in this project or any descendant |
| `context:<name>` | `context:@home` | Override active context for this query |
| `user:<name>` | `user:alice` | Override user filter for this query |
| `--future` | | Include tasks with a future `start` date |
| `--all` | | Disable all implicit filtering |

### Sync
- All task data lives as one TOML file per task in a git repository
- SQLite DB is a local cache at `.next.db`, rebuilt lazily before each command
- `next sync` pulls from the remote git repo, rebuilds the DB, then pushes local commits
- Offline edits accumulate as local git commits; sync merges them when connectivity returns
- Git merge conflicts are resolved manually

### Forgejo integration
- **Import** — pull issues from a Forgejo repository in as tasks
- **Completion sync** — marking an imported task complete closes the corresponding Forgejo issue
- Each imported task stores its Forgejo issue URL so duplicates are avoided on re-import

### WebCal (iCalendar) support
- **Import** — read a `.ics` / webcal feed and create or update tasks based on VTODO entries
- **Export** — write tasks as VTODO entries

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
