# Task Manager — Goals

This file captures rough goals and ideas to be refined into structured documentation later.

## What I want to build

A CLI task manager built around GTD (Getting Things Done). The tool should make it fast
to capture tasks without friction and then process them into the right stage. The primary
purpose of the tool is to surface *what to work on next* via automatic scoring, so the
user rarely has to think about ordering.

**Storage model**: tasks live as text files (JSON or TOML) committed to a git repository —
this is the source of truth and enables offline-first sync across machines. SQLite is a
derived cache rebuilt from those files for fast queries and scoring.

Output should be pipe-friendly (clean JSON / plain text) so the tool integrates naturally
with Claude Code and other CLI tools rather than embedding AI directly.

## GTD stages

- **Inbox** — default landing zone; tasks land here on capture, no metadata required
- **Projects** — multi-step outcomes; tasks can belong to a project
- **Waiting-for** — blocked on someone else; tracks who and since when
- **Someday/Maybe** — low-commitment ideas to revisit later

## Key features

### Capture & organisation
- Add a task to inbox in one command with minimal typing
- Move tasks from inbox to the right stage / project
- Due dates — optional deadline on any task
- Reminders — surface overdue or due-today tasks prominently on every run
- Priority — high / medium / low urgency on each task
- Tags / contexts — free-form labels (e.g. `@home`, `@work`) that represent the working
  environment; active contexts filter which tasks are shown
- Projects — group related tasks; project-level priority influences task scores

### Recurrence
Two distinct recurrence models:

- **Schedule-based** — task recurs on a fixed calendar rule (e.g. "1st of every month",
  "every Monday"). The next instance appears on the scheduled date regardless of when the
  previous one was completed.
- **Completion-based** — task recurs a fixed interval after the last completion (e.g.
  "water plants 7 days after last watered"). The next instance is created when the current
  one is marked done, with the due date calculated from the completion timestamp.

### Resources
Named toggles representing things that may not always be available (e.g. `printer`,
`vacation`). A resource can be marked available or unavailable globally. Tasks that
require an unavailable resource are excluded from scoring and the default task list.
Resources are separate from contexts: contexts describe where you are, resources describe
what you have access to.

### Nested tasks (subtasks)
A task can have subtasks to any depth. The parent task is treated as blocked until all of
its subtasks are completed — it is excluded from scoring and the default view until then.
Nesting is the primary organisation tool alongside projects.

### Blocking tasks
A task can declare that it is blocked by one or more other tasks. Blocked tasks are
excluded from scoring and the default view until all blockers are resolved.

### Automatic scoring
Urgency score computed per task (Taskwarrior-style) from:

- **Due date / urgency** — overdue and near-due tasks score higher
- **Priority level** — high/medium/low weight applied to the base score
- **Project priority** — parent project's priority multiplies or offsets the score
- **Age** — older tasks float up, *unless* the task is marked `long-term`, in which case
  age does not contribute to the score
- **User adjustment** — a manual numeric boost or penalty the user can apply to any task

The default `next` / `list` command ranks tasks by score descending, after filtering out
blocked tasks, tasks requiring unavailable resources, and tasks not matching the active
contexts.

### Sync
- All task data lives as text files (one file per task, or per project) in a git repo
- SQLite DB is a local cache rebuilt from the text files
- `sync` command: pull from remote git repo, rebuild DB, then push any local changes
- Offline edits accumulate as local git commits; sync merges them when connectivity returns

### Forgejo integration
- **Import** — pull issues from a Forgejo repository in as tasks, preserving title, body,
  labels, and open/closed state
- **Completion sync** — marking an imported task complete closes the corresponding Forgejo
  issue; no other fields are written back
- Each imported task stores its Forgejo issue URL so duplicates are avoided on re-import

### WebCal (iCalendar) support
- **Import** — read a `.ics` / webcal feed and create or update tasks based on VTODO
  entries; only task status (NEEDS-ACTION → open, COMPLETED / CANCELLED → done) is
  imported; other fields are ignored
- **Export** — write tasks as VTODO entries; maps title, due date, priority, and
  completion status; fields with no iCalendar equivalent are omitted gracefully

### Pipe-friendly output
Every list/show command supports a `--json` flag. Plain-text output is structured and
stable enough to pipe to Claude Code or other tools.

## Out of scope (for now)

- Built-in AI; AI is invoked externally via piped output
- GUI or TUI
- Multi-user / shared task lists

## Open questions

- **Text file format** — JSON (tooling-friendly) or TOML (human-editable)? Since files
  live in git and users may edit them manually, TOML leans slightly better.
- **Conflict resolution** — when two machines edit the same task file offline, git merge
  conflicts must be resolved manually. Is this acceptable, or do we need a CRDT/last-write-
  wins strategy?
- **DB rebuild trigger** — rebuild on every command, lazily on first use after a git pull,
  or explicitly via a `tm rebuild` command?
- **Recurrence instance creation** — for schedule-based tasks, does the next instance
  appear immediately at the start of the period, or N days before the due date?
- **Reminders mechanism** — just show overdue tasks on every run, or also support OS
  desktop notifications / a background daemon?
- **Project hierarchy** — flat list of projects, or can projects be nested?
- **Waiting-for tracking** — store the person being waited on as free text, or a
  structured contact field?
- **Weekly review workflow** — should there be an interactive `review` command that walks
  through each stage?
- **Database location** — `~/.local/share/task-manager/tasks.db` or configurable via env var?
- **Long-term task marker** — simple boolean flag, or a separate scheduling hint?
