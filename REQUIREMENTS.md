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
| `slug` | string | User-provided identifier (e.g. `"water-plants"`); must be unique |
| `parent_id` | UUID string | UUID of the parent task; used for subtasks and project membership |
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

### 1.2 Projects and subtasks

There is no separate project type. Any task can have child tasks by setting `parent_id`
on the children. Hierarchy nests to unlimited depth. The `stage = "project"` value is a
GTD workflow marker (meaning "active committed project") but is not required for a task
to act as a parent — any task at any stage can have subtasks.

A task's slug (e.g. `"work-infra"`) can be used instead of its UUID when specifying
`parent_id` or `blocked_by` on the command line. The tool resolves the slug to a UUID
before writing the TOML file.

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
    water-plants-a1b2c3d4.toml   # filename: <slug>.toml if slug set, else <title-slug>-<first-8-uuid>.toml
    work-infra.toml              # project task with slug "work-infra"
    deploy-db-e5f6a7b8.toml
  state.toml
  .gitignore                     # must include the DB path if it is inside the repo
```

All task files MUST reside in the flat `tasks/` directory. There is no `projects/`
directory; project tasks are stored alongside all other tasks.

File names MUST follow this rule: if the task has a `slug`, the file is named
`<slug>.toml`; otherwise `<title-slug>-<first-8-uuid>.toml` where the title slug is
lower-case with spaces replaced by `-` and non-alphanumeric characters stripped.
Example: `"Water plants"` with no slug and UUID `a1b2c3d4-…` → `water-plants-a1b2c3d4.toml`.

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

Any task can have child tasks via `parent_id`. A parent task is excluded from the
*default scored list* until all of its direct children have `status = done` or
`status = cancelled`; it does not appear in `next list` / `next next` output until then.

The user MAY mark a parent task done at any time via `next done <id>` regardless of
child task status — the completion gate only affects automatic scoring visibility, not
explicit user actions.

A parent task MUST NOT be automatically marked `done` when all subtasks complete; the
user marks it done explicitly.

Blocking is **not** transitive through depth: a grandparent is only blocked by its
direct children (who are in turn blocked by their own children).

Project tasks that are long-running SHOULD use `long_term = true` to suppress age-based
scoring while the project is in progress; this prevents them from rising to the top of
the list merely because they are old.

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
| `--slug <slug>` | User-provided identifier; must be unique; used as the filename |
| `--tag <tag>` | Repeatable |
| `--parent <id-or-slug>` | Sets `parent_id`; accepts UUID, UUID prefix, or slug |
| `--notes <text>` | |
| `--stage <stage>` | Default: `inbox` |
| `--recur schedule <rule>` | Creates a schedule-based recurring task |
| `--recur completion <days>` | Creates a completion-based recurring task |
| `--long-term` | Sets `long_term = true` |
| `--wait-for <who>` | Sets `waiting_for` and `stage = waiting` |
| `--blocked-by <id-or-slug>` | Repeatable; accepts UUID, UUID prefix, or slug |
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
next show <id-or-slug>             # full task details including subtasks and blockers
next done <id-or-slug>             # mark done; triggers recurrence if applicable
next cancel <id-or-slug>           # mark cancelled
next edit <id-or-slug> [options]   # modify fields (same options as add)
next delete <id-or-slug>           # permanently remove (prompts for confirmation)
next move <id-or-slug> --parent <id-or-slug> --stage <stage>  # relocate a task
```

All `<id-or-slug>` arguments MUST accept a full UUID, an unambiguous UUID prefix
(minimum 4 hex characters), or a task's slug.

### 8.4 Context and resource management

```
next context                        # show active contexts
next context set <@tag>...          # replace active context set
next context clear                  # clear all active contexts

next resource                       # list resources and availability
next resource set <$tag> on|off     # toggle a resource
```

### 8.5 Project commands

`next project` commands are convenience wrappers. Any task can have subtasks; `project`
commands simply default `--stage project` and present output in a tree view.

```
next project list                   # list tasks with stage=project (tree view via parent_id)
next project add <title> [options]  # shorthand for `next add --stage project`
next project show <id-or-slug>      # show the task and all its descendants (any stage)
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
