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

```
my-tasks/
  .git/
  .gitignore                   # contains ".next.db"
  tasks/
    call-dentist-a1b2c3d4.toml
    water-plants.toml          # task with slug "water-plants"
  state.toml                   # active contexts, active users, resource availability
  next.log                     # append-only command log (rotated at 1 MB)
  .next.db                     # SQLite read cache — not committed
```

---

## Projects and subtasks

Any task can act as a project. Mark a task as a project by giving it the `project` tag
(or use `next project add` as a shorthand). Subtasks attach via `--parent`. A parent
task is hidden from the default scored list until all its direct children are resolved.

```sh
next project add "Launch blog" --slug launch-blog
next add "Write first post" --parent launch-blog
next add "Set up hosting" --parent launch-blog
next project list
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
next context set @home        # global context filter
next context clear            # show all contexts

next resource set #printer off  # hide printer tasks
next resource set #printer on   # show them again
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

next list --all-users     # temporarily bypass the user filter
next list user:alice      # override for one query
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
| `--all-users` | | Bypass user filter |

---

## Command summary

| Command | Description |
|---------|-------------|
| `next init` | Initialise a task repository in the current directory |
| `next add` | Add a task |
| `next list` | List tasks sorted by urgency score |
| `next next [N]` | Show top N highest-scored tasks (default 10) |
| `next show <id>` | Full details of a single task |
| `next done <id>` | Mark done; triggers recurrence if applicable |
| `next cancel <id>` | Mark cancelled |
| `next edit <id>` | Modify fields on an existing task |
| `next delete <id>` | Permanently remove a task |
| `next move <id>` | Change parent task |
| `next open <id>` | Open the task's URL in the browser |
| `next project list/add/show` | Tree view of project tasks |
| `next data set/unset/get` | Manage arbitrary key-value data on a task |
| `next context [set/clear]` | Manage global context filter |
| `next resource [set]` | Manage resource availability |
| `next user [set/clear/list]` | Manage user filter |
| `next forecast` | Show upcoming due dates grouped by time |
| `next sync` | Pull from remote, push local commits |
| `next import forgejo` | Import issues from Forgejo (not yet implemented) |
| `next import ical` | Import VTODO from iCalendar (not yet implemented) |
| `next export ical` | Export tasks as iCalendar (not yet implemented) |

Task IDs accept a full UUID, a slug, or any unambiguous 4+ character hex prefix.
All commands support `--json` for pipe-friendly output.

---

## Backend configuration

By default `next` stores tasks locally. A remote HTTP backend can be configured:

```toml
# $XDG_CONFIG_HOME/task-manager/config.toml
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
- `ARCHITECTURE.md` — internal design
- `CLI.md` — full command reference
- `CRATES.md` — workspace structure
