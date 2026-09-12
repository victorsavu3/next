# Task Manager — Goals

This file is the durable philosophy/vision doc: the "why" behind the tool, kept alongside
the structured documentation (`README.md`, `REQUIREMENTS.md`, `ARCHITECTURE.md`, `CLI.md`,
`TUI.md`, `TUTORIAL.md`) rather than superseded by it.

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
sets scores or data fields externally. That interface is not limited to piping `--json`
through a shell: the optional `next-mcp` server exposes the same operations as MCP tools
over HTTP/JSON-RPC, for AI clients that talk MCP directly instead of shelling out.

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

| Prefix | Kind | Example | What it names |
|--------|------|---------|---------------|
| `@` | Context | `@home`, `@work` | A working environment |
| `#` | Resource | `#printer`, `#vacation` | Something that must be available |
| *(none)* | Freeform | `python`, `project` | Anything else |

The prefix is a naming convention only. **Every tag filters the same way**, through one
machine-local state: a tag is `required`, `excluded`, or `accepted` (e.g.
`next tag require @home`, `next tag exclude '#printer'`). While anything is required,
only tasks carrying a required tag are shown; a task carrying an excluded tag is hidden
whatever else it carries. State is inherited by nested tags, and an explicit `accepted`
lets a child opt out of its parent's state.

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

These are the factors, not fixed numbers: the weights behind them live in a committed
`config/scoring.toml` (seeded by `next init`), so every consumer — CLI, MCP server,
Forgejo plugin — scores from the same repository-tracked values, and a user can retune
them without a code change.

The default `next` / `list` command ranks tasks by score descending, after filtering out
blocked tasks, tasks with unavailable `#` tags, and tasks not matching the active `@`
contexts. Tasks with a `start` date in the future are hidden entirely until that date.

### Query interface
All list commands accept filters that can be combined freely:

| Filter | Example | Meaning |
|--------|---------|---------|
| `+tag` | `+@home`, `+python` | Task must have this tag |
| `-tag` | `-@work` | Task must not have this tag |
| `parent:<slug>` | `parent:work-infra` | Task is in this project or any descendant |
| `context:<name>` | `context:@home` | Include exactly this tag for this query, ignoring the stored inclusions |
| `user:<name>` | `user:alice` | Override user filter for this query |
| `--future` | | Include tasks with a future `start` date |
| `--all` | | Disable all implicit filtering |

### Sync
- All task data lives as one TOML file per task in a git repository
- SQLite DB is a local cache at `.next.db`, rebuilt lazily before each command
- `next sync` pulls from the remote git repo, rebuilds the DB, then pushes local commits
- Offline edits accumulate as local git commits; sync merges them when connectivity returns
- Git merge conflicts are resolved manually
- During sync, closed tasks past an age threshold are archived automatically into
  month-keyed segment files under `archive/`, keeping the working set small without
  losing history — see REQUIREMENTS.md §2.3

### Forgejo integration (plugin — implemented)
Provided by the bundled `next-forgejo` binary behind the off-by-default `forgejo`
feature: maps Forgejo repos to contexts, imports open issues as tasks, and syncs
resolution both ways. See REQUIREMENTS.md §10.3.

### WebCal (iCalendar) support (removed from core binary)
Built-in iCal/WebCal import and export has been removed from `next` and will be
provided as an external plugin. See REQUIREMENTS.md §10.

### Pipe-friendly output
Every list/show command supports a `--json` flag. Plain-text output is structured and
stable enough to pipe to Claude Code or other tools.

## Out of scope (for now)

- Built-in AI; AI is invoked externally via piped output
- GUI (a terminal UI, `next-tui`, has since been implemented — see TUI.md)
- Multi-user / shared task lists
- OS desktop notifications or background reminder daemon

## Open questions

- **Recurrence — lead time** — for schedule-based tasks, should the next instance become
  visible immediately after the previous one completes, or only when today reaches the due
  date? (Current assumption: visible immediately, scored by due date proximity.)
- **Subtask files** — should subtasks be embedded in the parent's TOML file or stored as
  separate files with a `parent` reference? Separate files are better for git diffs;
  embedded is simpler to edit by hand.
