# next

A CLI task manager with automatic urgency scoring. Tasks are stored as TOML files in a
git repository, enabling offline-first sync across machines.

---

## Quick start

```sh
# One-time setup: initialise a task repository
mkdir ~/tasks && cd ~/tasks
next init

# Capture a task
next add "Call dentist"

# Add with a deadline and context tag
next add "Submit tax forms" --due "April 15" --tag @home --priority high

# What should I work on now?
next next

# See the full list
next list

# Mark done
next done a1b2c3d4
```

---

## Storage model

Every task is a `.toml` file inside a `tasks/` directory at the repository root. `git`
is the transport layer — `next sync` runs pull then push. The TOML files are the single
source of truth.

`next` also maintains an SQLite database (`.next.db`) as a read cache. It is rebuilt
automatically whenever the git HEAD changes (e.g. after a pull), so it is always
consistent with the TOML files. Add it to `.gitignore`; `next init` does this for you.

Machine-local state (active contexts, active users, resource availability) is stored
outside the repository in `$XDG_STATE_HOME/task-manager/<repo-hash>/state.toml` so it
is never committed or synced.

```
my-tasks/
  .git/
  .gitignore                   # contains ".next.db"
  tasks/
    call-dentist-a1b2c3d4.toml
    water-plants.toml          # task with slug "water-plants"
  tags/
    @work.toml                 # tag description for @work
    @home/
      kitchen.toml             # tag description for @home/kitchen
  next.log                     # append-only command log (rotated at 1 MB)
  .next.db                     # SQLite read cache — not committed

~/.local/state/task-manager/<repo-hash>/
  state.toml                   # machine-local: active contexts, users, resource availability
```

---

## Projects and subtasks

Any task can have subtasks. Give a task the `project` tag to mark it as a project.
Subtasks attach via `--parent`. A parent task is hidden from the default scored list
until all its direct children are resolved.

```sh
next add "Launch blog" --slug launch-blog --tag project
next add "Write first post" --parent launch-blog
next add "Set up hosting" --parent launch-blog
next tree                      # show all tasks in a parent-child tree
next show launch-blog          # show full details for a single task
```

---

## Tag system

All labels on a task are tags. Prefix conventions give some tags special meaning:

| Prefix | Kind | Example | Effect |
|--------|------|---------|--------|
| `@` | Context | `@home`, `@work` | Hidden when a different context is active |
| `#` | Resource | `#printer`, `#vacation` | Hidden when resource is unavailable |
| *(none)* | Freeform | `python`, `project` | No implicit filter |

Tasks with no `@` tag are always shown regardless of the active context.

```sh
next context set @home          # global context filter
next context clear              # show all contexts

next resource set #printer off  # hide printer tasks
next resource set #printer on   # show them again
```

### Tag descriptions

Any tag (context, resource, or freeform) can carry a human-readable description. These
descriptions are stored as individual TOML files under `tags/` in the repository and are
committed to git, making them visible to all machines. They appear in `next tag`,
`next context`, and `next resource` output and serve as structured metadata for AI agents
reading the repository.

```sh
next tag describe @work "Tasks at the standing desk — laptop required"
next context describe @home "Home tasks: kitchen, garden, errands"
next resource describe #printer "Office laser printer, 2nd floor"
next tag                        # list all tags with their descriptions
```

---

## User filter

The user filter is for task organisation, not access control. When active users are set,
tasks assigned to other users are hidden. Unassigned tasks are always visible (shared
backlog).

```sh
next user set alice bob   # only show tasks for alice and bob
next user clear           # no user filter
next user list            # all assignees across all tasks
```

---

## Urgency scoring

Every visible task receives a numeric score used for ranking:

```
score = due_factor + priority_factor + project_factor + age_factor + score_adjustment
```

- **Due factor** — rises sharply as the deadline approaches; peaks when overdue
- **Priority** — `high` (+2), `medium` (+1), `low` (0)
- **Project factor** — parent task's priority offsets the score (+0.5 / 0.0 / −0.5)
- **Age** — older tasks float up; capped at +2; zeroed when `long_term = true`
- **Adjustment** — manual boost/penalty via `--adjust`

---

## Filter tokens

All list commands accept filter tokens in any order:

| Token | Example | Meaning |
|-------|---------|---------|
| `+<tag>` | `+@home`, `+python` | Task must have this tag |
| `-<tag>` | `-@work` | Task must not have this tag |
| `project:<path>` | `project:work` | Task belongs to this project tree |
| `context:<@tag>` | `context:@home` | Override active context for this query |
| `user:<name>` | `user:alice` | Override user filter for this query |
| `--future` | | Include tasks with a future `start` date |
| `--all` | | Disable all implicit filtering |

---

## Command summary

| Command | Description |
|---------|-------------|
| `next init` | Initialise a task repository in the current directory |
| `next add` | Add a task |
| `next list [-n N]` | List tasks sorted by urgency score; `-n`/`--limit` caps output |
| `next next [N]` | Show top N highest-scored tasks (default 10) |
| `next show <id>` | Full details of a single task |
| `next tree` | Show all tasks in a parent-child tree |
| `next start <id>` | Mark as started (in-progress); logs a time entry |
| `next stop <id>` | Stop a started task (returns to open); logs a time entry |
| `next done <id>` | Mark done; triggers recurrence if applicable |
| `next cancel <id>` | Mark cancelled |
| `next edit <id>` | Modify fields on an existing task |
| `next delete <id>` | Permanently remove a task |
| `next move <id>` | Change parent task |
| `next open <id>` | Open the task's URL in the browser |
| `next data set/unset/get` | Manage arbitrary key-value data on a task |
| `next tag [describe/clear-description]` | List tags with descriptions; set/remove a tag description |
| `next context [set/clear/describe/clear-description]` | Manage global context filter and descriptions |
| `next resource [set/describe/clear-description]` | Manage resource availability and descriptions |
| `next user [set/clear/list]` | Manage user filter |
| `next forecast` | Show upcoming due dates grouped by time |
| `next sync` | Pull from remote, push local commits |
| `next import forgejo` | Import issues from Forgejo (not yet implemented) |
| `next import ical` | Import VTODO from iCalendar (not yet implemented) |
| `next export ical` | Export tasks as iCalendar (not yet implemented) |

Task IDs accept a full UUID, a slug, or any unambiguous 4+ character hex prefix.
All commands support `--json` for pipe-friendly output.

The `--autosync` global flag (or `autosync = true` in the config file) automatically
runs `next sync` after every mutation command.

---

## Backend configuration

By default `next` stores tasks locally. The config file lives at
`$XDG_CONFIG_HOME/task-manager/config.toml`:

```toml
repository = "/home/alice/tasks"  # use next from any directory

autosync   = true                 # sync automatically after each mutation
list_limit = 20                   # cap `next list` output (same as -n 20)

[backend]
kind = "remote"

[backend.remote]
url   = "https://tasks.example.com"
token = "my-bearer-token"   # optional
```

The remote backend is a stub — it returns "not yet implemented" errors. The local backend
is the production-ready path.

---

## See also

- `REQUIREMENTS.md` — functional requirements
- `ARCHITECTURE.md` — internal design and module layout
- `CLI.md` — full command reference
