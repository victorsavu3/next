# Requirements

Precision level: MUST = mandatory, SHOULD = strongly preferred, MAY = optional.

---

## 1. Data model

### 1.1 Task fields

Each task MUST carry the following fields:

| Field | Type | Notes |
|-------|------|-------|
| `id` | UUID v4 string | Assigned on creation, never changed |
| `title` | non-empty string | |
| `status` | `open` \| `done` \| `cancelled` | |
| `stage` | `inbox` \| `project` \| `waiting` \| `someday` | |
| `created_at` | RFC 3339 datetime | Set on creation |
| `updated_at` | RFC 3339 datetime | Updated on every write |

A task MAY carry the following optional fields:

| Field | Type | Notes |
|-------|------|-------|
| `priority` | `low` \| `medium` \| `high` | Default: `medium` |
| `due` | date string (YYYY-MM-DD) | Optional deadline |
| `start` | date string (YYYY-MM-DD) | Task hidden until this date; disables age scoring |
| `long_term` | boolean | Disables age scoring; default `false` |
| `project` | string path | `/`-separated, e.g. `"work/infra"` |
| `parent_id` | UUID string | Declares this task a subtask of another |
| `tags` | array of strings | See §3 |
| `waiting_for` | string | Free-text; meaningful when `stage = "waiting"` |
| `blocked_by` | array of UUID strings | Explicit blockers |
| `score_adjustment` | float | Added directly to computed score |
| `notes` | string | Multi-line free text |

External-reference fields (set by importers, never by the user):

| Field | Type | Notes |
|-------|------|-------|
| `forgejo_issue` | URL string | Set by Forgejo importer |
| `webcal_uid` | string | iCalendar UID; set by iCal importer |

When a task recurs it MUST carry a `[recurrence]` table:

```toml
[recurrence]
type = "schedule"       # "schedule" | "completion"
rule = "every Monday"   # human-readable rule for schedule-based recurrence
interval_days = 7       # positive integer; for completion-based recurrence
```

Exactly one of `rule` or `interval_days` MUST be present in a `[recurrence]` table.

### 1.2 Project metadata

Projects MUST be defined by TOML files inside the `projects/` directory. A project at
path `work/infra` MUST have its metadata at `projects/work/infra.toml`.

A project MUST have:

| Field | Type |
|-------|------|
| `name` | string |
| `created_at` | RFC 3339 datetime |

A project MAY have:

| Field | Type | Notes |
|-------|------|-------|
| `priority` | `low` \| `medium` \| `high` | Default: `medium` |
| `description` | string | |

### 1.3 Global state

Global state MUST be stored in `state.toml` at the repository root.

```toml
active_contexts = ["@home"]                       # active @ tags (empty = no filter)
[resources]
printer = true
vacation = false
```

---

## 2. Storage

### 2.1 Repository layout

```
<repo-root>/
  tasks/
    water-plants-a1b2c3d4.toml   # filename: <title-slug>-<first-8-of-uuid>.toml
    deploy-db-e5f6a7b8.toml
  projects/
    work.toml
    work/
      infra.toml
  state.toml
  .gitignore                     # must include the DB path if it is inside the repo
```

All task files MUST reside in the flat `tasks/` directory regardless of project
membership or parent/child relationships. Subtasks are separate files with a `parent_id`
field — they are NOT embedded in the parent file.

File names MUST be derived from the task title (lower-case, spaces replaced with `-`,
non-alphanumeric characters stripped) followed by `-` and the first 8 hex characters of
the UUID. Example: `"Water plants"` with UUID `a1b2c3d4-…` → `water-plants-a1b2c3d4.toml`.

### 2.2 SQLite cache

The SQLite database MUST be stored at `$XDG_CACHE_HOME/task-manager/cache.db`,
defaulting to `~/.cache/task-manager/cache.db`. It MUST NOT be inside the git
repository.

Before executing any command, the tool MUST compare the git `HEAD` hash recorded in the
DB against the current `HEAD`. If they differ or the DB does not exist, the tool MUST
rebuild the DB from the TOML files before proceeding.

The rebuild MUST be a full replace (drop and re-import all rows) to avoid drift.

### 2.3 Sync

`next sync` MUST execute the following steps in order, stopping on any error:

1. `git pull` from the configured remote (fast-forward or merge)
2. If merge conflicts exist, print an actionable error message and exit with code 2
3. Rebuild the SQLite cache
4. `git push` local commits to the remote

All mutations (add, edit, done, import) MUST produce a git commit automatically. The
commit message MUST identify the operation and the task title.

---

## 3. Tag system

Tags are strings in the `tags` array of a task. Prefix conventions:

| Prefix | Kind | Example |
|--------|------|---------|
| `@` | Context | `@home`, `@work` |
| `$` | Resource | `$printer`, `$vacation` |
| *(none)* | Freeform | `python`, `reading` |

### 3.1 Context filtering

When one or more `@` contexts are active:
- Tasks with **no** `@` tags MUST always be included
- Tasks with at least one `@` tag MUST be included only if they share at least one `@` tag with the active set
- Tasks whose `@` tags are all outside the active set MUST be excluded

When **no** context is active, all tasks MUST be shown regardless of their `@` tags.

A query-time `context:@name` filter MUST override the global active-context set for that
single invocation.

### 3.2 Resource filtering

A resource `$<name>` is available when `resources.<name> = true` in `state.toml` (absent
keys default to **available**).

Tasks that carry a `$<name>` tag where `<name>` is **unavailable** MUST be excluded from
the default list and from score computation.

---

## 4. Scoring

Every task that passes filtering MUST be assigned a numeric urgency score used to rank
the default output.

The score MUST be the sum of the following weighted factors:

| Factor | Condition |
|--------|-----------|
| **Due proximity** | Always; rises as due date approaches; highest value when overdue |
| **Priority** | Always; `low` / `medium` / `high` map to fixed additive weights |
| **Project priority** | When `project` is set; project's priority adds an offset |
| **Age** | Only when `long_term = false` AND (`start` is unset OR `start` ≤ today) |
| **User adjustment** | Always; `score_adjustment` added directly |

Default weights MUST be defined in code and SHOULD be overridable via a user config file
at `$XDG_CONFIG_HOME/task-manager/config.toml`.

Tasks excluded from the default view (blocked, resource-unavailable, future `start`,
parent awaiting subtasks) MUST NOT receive a score and MUST NOT appear in `next list` /
`next next` output unless `--all` is passed.

---

## 5. Filtering and queries

All list commands MUST accept the following filter tokens, freely combinable:

| Syntax | Meaning |
|--------|---------|
| `+<tag>` | Task must have this tag |
| `-<tag>` | Task must not have this tag |
| `project:<path>` | Task is in this project or any descendant |
| `context:<@tag>` | Use this context instead of the active set for this query |
| `--future` | Include tasks with future `start` date and planned recurrence instances |
| `--all` | Disable all implicit filtering (contexts, resources, blocked, start date) |
| `--stage <stage>` | Restrict to one GTD stage |

Multiple `+tag` tokens MUST be combined with AND (task must have all of them).
Multiple `-tag` tokens MUST be combined with AND (task must have none of them).

---

## 6. Blocking and subtasks

### 6.1 Explicit blockers

A task with non-empty `blocked_by` MUST be excluded from the default list and scoring
until every referenced task has `status = done` or `status = cancelled`.

### 6.2 Subtasks

A task with a `parent_id` is a subtask. The parent task MUST be treated as blocked by
all of its direct children — it is excluded from the default list and scoring until every
direct child has `status = done` or `status = cancelled`.

Blocking is **not** transitive through subtask depth: a grandparent is only blocked by
its direct children, not by grandchildren (the child itself is blocked by the grandchild,
which in turn blocks the grandparent).

A parent task MUST NOT be automatically marked `done` when all subtasks complete. The
user MUST mark the parent done explicitly.

---

## 7. Recurrence

### 7.1 Completion-based

When `next done` is run on a task with `recurrence.type = "completion"`:

1. The current task is marked `done`
2. A new task MUST be created, copying all fields from the completed task except `id`,
   `status` (`open`), `created_at`, `updated_at`, and `due`
3. The new task's `due` MUST be set to `completed_at_date + recurrence.interval_days`

### 7.2 Schedule-based

A schedule-based recurrence template MUST maintain a single active instance at all times.
When `next done` is run on the active instance:

1. The current task is marked `done`
2. A new task MUST be created immediately with `due` set to the next date produced by
   `recurrence.rule` after the current due date
3. The new instance MUST be visible in the default list right away and scored by its
   due-date proximity

The recurrence template MUST be identifiable (e.g. via a shared `recurrence_id` field on
all instances) so `next forecast` can project future occurrences.

`next forecast` MUST accept the same filter tokens as `next list` and MUST display the
upcoming due dates for all matching recurrence series for a configurable horizon
(default: 90 days).

---

## 8. CLI commands

The binary MUST be named `next`. All commands MUST support `--json` to emit JSON output.
Exit codes: `0` success, `1` user/input error, `2` system error.

### 8.1 `next add`

```
next add <title> [options]
```

| Option | Notes |
|--------|-------|
| `--due <expr>` | Natural-language date accepted ("in two weeks", "next Monday") |
| `--start <expr>` | Natural-language date accepted |
| `--priority high\|medium\|low` | |
| `--project <path>` | |
| `--tag <tag>` | Repeatable |
| `--parent <id>` | Makes this task a subtask |
| `--notes <text>` | |
| `--stage <stage>` | Default: `inbox` |
| `--recur schedule <rule>` | Creates a schedule-based recurring task |
| `--recur completion <days>` | Creates a completion-based recurring task |
| `--long-term` | Sets `long_term = true` |
| `--wait-for <who>` | Sets `waiting_for` and `stage = waiting` |
| `--blocked-by <id>` | Repeatable |
| `--adjust <float>` | Sets `score_adjustment` |

### 8.2 `next list` and `next next`

```
next list [filters...]    # full scored list; overdue/due-today shown first with emphasis
next next [N] [filters...]  # top N tasks by score (default N=10)
```

Both commands MUST show overdue and due-today tasks visually distinct (e.g. coloured or
prefixed) at the top of the output.

### 8.3 Task actions

```
next show <id>             # full task details including subtasks and blockers
next done <id>             # mark done; triggers recurrence if applicable
next cancel <id>           # mark cancelled
next edit <id> [options]   # modify fields (same options as add)
next delete <id>           # permanently remove (prompts for confirmation)
next move <id> --project <path> --stage <stage>  # relocate a task
```

### 8.4 Context and resource management

```
next context                        # show active contexts
next context set <@tag>...          # replace active context set
next context clear                  # clear all active contexts

next resource                       # list resources and availability
next resource set <$tag> on|off     # toggle a resource
```

### 8.5 Project management

```
next project list                   # list all projects (tree view)
next project add <path> [options]   # create project (--priority, --description)
next project show <path>            # show project metadata and its tasks
```

### 8.6 Review

```
next review
```

Walks through GTD stages in order: **inbox → waiting-for → someday/maybe → projects**.
For each item the tool presents the task and prompts the user to choose an action (e.g.
process, skip, move, done, delete). MUST be interruptible (Ctrl-C leaves tasks unchanged).

### 8.7 Sync

```
next sync
```

See §2.3.

### 8.8 Forecasting

```
next forecast [filters...]
```

See §7.2.

---

## 9. Integrations

### 9.1 Forgejo

```
next import forgejo <owner/repo> [--project <path>] [--tag <tag>]
```

- MUST fetch all issues (open and closed) from the Forgejo API
- MUST create a task for each issue not already present (matched by `forgejo_issue` URL)
- MUST update `status` of previously imported issues to reflect current Forgejo state
- MUST map issue labels to freeform tags on the task
- MUST store the issue URL in `forgejo_issue`
- MUST NOT overwrite user edits to other fields on re-import

When `next done` is called on a task with a `forgejo_issue` field, the tool MUST close
the corresponding issue via the Forgejo API. No other field is written back to Forgejo.

### 9.2 iCalendar (WebCal)

```
next import ical <file-or-url>
next export ical [filters...] [--output <file>]
```

**Import:**
- MUST parse VTODO components from the provided `.ics` file or URL
- MUST create or update tasks matched by `webcal_uid`
- MUST only import the `STATUS` field: `NEEDS-ACTION` / `IN-PROCESS` → `open`;
  `COMPLETED` / `CANCELLED` → `done` / `cancelled`
- MUST NOT import any other VTODO fields

**Export:**
- MUST emit a valid iCalendar file with one VTODO per matching task
- MUST map: `title` → `SUMMARY`, `due` → `DUE`, `priority` → `PRIORITY`,
  `status` → `STATUS`
- MUST emit `UID` using the task's UUID
- Fields with no iCalendar equivalent MUST be omitted silently

---

## 10. Output

- Every list/show command MUST support `--json` emitting valid, stable JSON
- Plain-text output SHOULD use colour when stdout is a TTY; MUST fall back to plain
  text when stdout is not a TTY (pipe-safe)
- Date expressions in `--due` and `--start` MUST accept natural-language input
  ("tomorrow", "in two weeks", "next Monday", "2026-06-01") in addition to ISO 8601
