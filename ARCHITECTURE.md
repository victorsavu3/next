# Architecture

This document describes the internal design of `tm`. Read `REQUIREMENTS.md` for the
"what"; this document covers the "how" and the "why".

---

## 1. Component overview

```
┌─────────────────────────────────────────────────────────┐
│                        tm binary                        │
│                                                         │
│  ┌──────────┐   ┌──────────────┐   ┌────────────────┐  │
│  │   CLI    │──▶│    Domain    │──▶│    Storage     │  │
│  │ (clap)   │   │  (pure Rust) │   │ TOML + SQLite  │  │
│  └──────────┘   └──────┬───────┘   └───────┬────────┘  │
│                        │                   │            │
│  ┌──────────┐   ┌──────▼───────┐   ┌───────▼────────┐  │
│  │  Output  │◀──│   Filter /   │   │   Git layer    │  │
│  │text/JSON │   │   Scoring    │   │  (git2 crate)  │  │
│  └──────────┘   └──────────────┘   └────────────────┘  │
│                                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │               Integrations                       │   │
│  │   Forgejo (HTTP/REST)   │   iCalendar (ical)     │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

The domain layer is pure logic with no I/O. Storage and integrations call into domain
types but never into each other. The CLI layer wires them together.

---

## 2. Crate dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `clap` | 4.x (derive) | CLI argument parsing |
| `serde` | 1.x | Serialisation for TOML, JSON |
| `toml` | 0.8.x | TOML file reading/writing |
| `serde_json` | 1.x | JSON output |
| `rusqlite` | 0.31.x | SQLite cache (synchronous) |
| `uuid` | 1.x (v4, serde) | Task and recurrence IDs |
| `chrono` | 0.4.x | Datetime types throughout |
| `interim` | 0.2.x | Natural-language date parsing ("in two weeks") |
| `rrule` | 0.14.x | RFC 5545 recurrence rule evaluation |
| `git2` | 0.19.x | Git operations (commit, pull, push) |
| `reqwest` | 0.12.x (blocking) | HTTP for Forgejo API and webcal URLs |
| `icalendar` | 0.16.x | iCalendar VTODO parsing and generation |
| `dialoguer` | 0.11.x | Interactive prompts for `tm review` |
| `owo-colors` | 4.x | Terminal colour (TTY-aware) |
| `dirs` | 5.x | XDG base directory resolution |
| `anyhow` | 1.x | Error propagation |
| `thiserror` | 1.x | Typed domain errors |

Rationale for key choices:

- **`chrono` over `jiff`** — `interim` and `rrule` both produce chrono types natively.
  `jiff` has a cleaner API but its ecosystem support is still maturing; revisit when
  `rrule` gains jiff support.
- **`rusqlite` over `sqlx`** — `tm` is synchronous; `rusqlite` avoids an async runtime.
- **`git2` over `Command("git")`** — avoids shell-injection risk and PATH dependency;
  gives programmatic access to conflict detection and index state.

---

## 3. Module structure

```
src/
  main.rs                  # entry point: parse args, call command handlers
  error.rs                 # AppError enum; maps to exit codes 0/1/2
  config.rs                # load ~/.config/task-manager/config.toml; scoring weights

  cli/
    mod.rs                 # top-level Cli struct (clap derive)
    filter.rs              # FilterArgs: shared filter flags parsed into FilterSet
    commands/
      add.rs
      list.rs              # handles both `list` and `next`
      show.rs
      done.rs
      edit.rs
      delete.rs
      move_.rs
      context.rs
      resource.rs
      project.rs
      review.rs
      sync.rs
      forecast.rs
      import_forgejo.rs
      import_ical.rs
      export_ical.rs

  domain/
    task.rs                # Task struct + validation; TomlTask for serde
    state.rs               # GlobalState (active contexts, resource map)
    tag.rs                 # TagKind enum (Context/Resource/Freeform); tag parsing
    filter.rs              # FilterSet struct; fn apply(tasks, filter) -> Vec<Task>
    scoring.rs             # fn score(task, parent_task, weights) -> f64
    recurrence.rs          # RecurrenceRule; fn next_due(rule, after) -> NaiveDate
                           # fn to_rrule(rule_str) -> RRuleSet
    date_parse.rs          # fn parse_date(expr, today) -> Result<NaiveDate>
                           # thin wrapper over `interim`

  storage/
    toml_store.rs          # fn read_task(path), write_task(task, repo_root)
                           # fn read_all_tasks(repo_root) -> Vec<Task>
                           # fn read_state(repo_root), write_state(...)
    db.rs                  # schema creation; fn rebuild(tasks, head)
                           # fn is_stale(db, repo_root) -> bool
                           # query helpers used by CLI commands
    git.rs                 # fn commit(repo, message); fn pull(repo); fn push(repo)
                           # fn current_head(repo) -> String
                           # fn has_conflicts(repo) -> bool

  output/
    text.rs                # fn render_task_list(tasks, opts); fn render_task(task)
    json.rs                # fn render_task_list_json(tasks); fn render_task_json(task)
```

---

## 4. Command execution lifecycle

Every command follows this sequence:

```
1. Parse CLI args (clap)
2. Locate repository root (walk up from CWD looking for state.toml / .git)
3. Open SQLite cache; check if stale (compare stored HEAD to current HEAD)
4. If stale: rebuild cache from TOML files
5. Execute command logic (reads from DB; writes to TOML files + DB)
6. If mutation: git commit the changed TOML files
7. Render output (text or JSON to stdout)
```

Read-only commands (list, show, forecast) skip steps 5b and 6.

The repository root is stored in a `RepoContext` struct passed through all layers so
paths are never computed ad hoc.

---

## 5. Storage layer

### 5.1 TOML file conventions

- One `.toml` file per task in a flat `tasks/` directory (no `projects/` directory)
- Filename: `<slug>.toml` if the task has a user slug; otherwise `<title-slug>-<first-8-uuid-hex>.toml`
- Title slug: lowercase, spaces → `-`, strip non-alphanumeric except `-`
- Files are written with `toml::to_string_pretty` for human readability

The `[recurrence]` table is only present when the task recurs; optional fields are
omitted rather than written as empty strings or nulls.

### 5.2 SQLite schema

```sql
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
    -- stores: git_head, schema_version
);

CREATE TABLE tasks (
    id                       TEXT PRIMARY KEY,
    title                    TEXT NOT NULL,
    status                   TEXT NOT NULL,   -- open|done|cancelled
    stage                    TEXT NOT NULL,   -- inbox|project|waiting|someday
    priority                 TEXT NOT NULL DEFAULT 'medium',
    due                      TEXT,            -- YYYY-MM-DD
    start_date               TEXT,            -- YYYY-MM-DD
    long_term                INTEGER NOT NULL DEFAULT 0,
    slug                     TEXT UNIQUE,     -- user-provided identifier
    parent_id                TEXT,            -- UUID of parent task (any stage)
    waiting_for              TEXT,
    score_adjustment         REAL NOT NULL DEFAULT 0.0,
    notes                    TEXT,
    forgejo_issue            TEXT,
    webcal_uid               TEXT,
    recurrence_type          TEXT,            -- schedule|completion
    recurrence_rule          TEXT,
    recurrence_interval_days INTEGER,
    recurrence_id            TEXT,            -- shared UUID across a series
    created_at               TEXT NOT NULL,
    updated_at               TEXT NOT NULL
);

CREATE TABLE task_tags (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    tag     TEXT NOT NULL,
    PRIMARY KEY (task_id, tag)
);

CREATE TABLE task_blockers (
    task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    blocked_by   TEXT NOT NULL REFERENCES tasks(id),
    PRIMARY KEY (task_id, blocked_by)
);

-- No projects table: project tasks are plain tasks with stage='project'

-- Useful indexes
CREATE INDEX idx_tasks_status ON tasks(status);
CREATE INDEX idx_tasks_stage  ON tasks(stage);
CREATE INDEX idx_tasks_parent ON tasks(parent_id);
```

### 5.3 Cache rebuild

`db::is_stale` reads `meta` key `git_head` and compares to `git2::Repository::head()
.peel_to_commit().id().to_string()`. A rebuild:

1. Opens a transaction
2. `DELETE FROM tasks; DELETE FROM task_tags; DELETE FROM task_blockers;`
3. Iterates all TOML files, inserts rows
4. Updates `meta` with current HEAD and schema version
5. Commits

Rebuild runs in-process; for a typical personal task set (hundreds of tasks) this takes
under 100 ms.

### 5.4 Git operations

`git.rs` wraps `git2`:

```rust
fn commit(repo: &Repository, files: &[PathBuf], message: &str) -> Result<()>
fn pull(repo: &Repository) -> Result<ConflictStatus>
fn push(repo: &Repository) -> Result<()>
fn current_head(repo: &Repository) -> Result<String>
```

All mutations call `git::commit` after writing TOML. Commit messages follow the pattern:
`tm: <verb> "<task title>"` — e.g. `tm: add "Water plants"`.

`git::pull` returns `ConflictStatus::Clean` or `ConflictStatus::Conflicts(Vec<PathBuf>)`.
The sync command aborts and prints the conflicting file paths when conflicts are detected.

---

## 6. Scoring

Scores are computed at query time (not stored in the DB) by `domain::scoring::score`.

```
score(task) =
    due_factor(task.due, today)
  + priority_factor(task.priority)
  + project_factor(project.priority)   -- 0.0 if no project
  + age_factor(task.created_at, today, task.long_term, task.start_date)
  + task.score_adjustment
```

### Default weights (all overridable in `config.toml`)

**`due_factor`** — models urgency as a curve:

| Condition | Value |
|-----------|-------|
| No due date | `0.0` |
| Overdue by N days | `12.0 + N × 0.3` |
| Due today | `12.0` |
| Due in 1–7 days | `6.0 + (7 − days) × 0.8` |
| Due in 8–30 days | `3.0 + (30 − days) × 0.1` |
| Due in 31+ days | `max(0.0, 2.0 − days × 0.01)` |

**`priority_factor`**:

| Priority | Value |
|----------|-------|
| `low` | `0.0` |
| `medium` | `1.0` |
| `high` | `2.0` |

**`project_factor`**:

| Project priority | Value |
|-----------------|-------|
| `low` | `−0.5` |
| `medium` | `0.0` |
| `high` | `+0.5` |

**`age_factor`**: `min(age_days × 0.01, 2.0)` — capped so old tasks never dominate.
Returns `0.0` when `long_term = true` or when `start_date > today` (task not yet active).

---

## 7. Recurrence

### 7.1 Completion-based

Stored as `recurrence.interval_days`. When `tm done` completes the task:

```rust
let next_due = completed_date + Duration::days(task.recurrence_interval_days);
let next = task.clone_as_new(next_due);  // new UUID, status=open, due=next_due
storage::write_task(&next, repo_root)?;
git::commit(repo, &[next.path()], &format!("tm: recur \"{}\"", task.title))?;
```

### 7.2 Schedule-based

Stored as `recurrence.rule` — a human-readable string that the user types. The
`recurrence` module translates it to an RFC 5545 `RRuleSet` at runtime.

**Translation layer** (`domain::recurrence::to_rrule`):

A small pattern-matching function converts common English expressions to RRULE strings.
This is intentionally a closed set — unknown patterns return an error at `tm add` time so
the user sees the failure immediately rather than at recurrence time.

| User input (examples) | RRULE string |
|-----------------------|-------------|
| `"every day"` | `FREQ=DAILY` |
| `"every weekday"` | `FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR` |
| `"every Monday"` | `FREQ=WEEKLY;BYDAY=MO` |
| `"every 2 weeks"` | `FREQ=WEEKLY;INTERVAL=2` |
| `"1st of every month"` | `FREQ=MONTHLY;BYMONTHDAY=1` |
| `"last day of month"` | `FREQ=MONTHLY;BYMONTHDAY=-1` |
| `"every year"` | `FREQ=YEARLY` |
| `"every January 1"` | `FREQ=YEARLY;BYMONTH=1;BYMONTHDAY=1` |

The natural-language `rule` string is stored verbatim in the TOML file (human-readable).
The RRULE conversion happens only in-process. All instances of a series share a
`recurrence_id` (UUID) so `tm forecast` can find them.

`fn next_due(rule_str, after_date) -> Result<NaiveDate>`:

```rust
let dtstart = after_date.and_hms_opt(0, 0, 0).unwrap().and_utc();
let rrule_str = format!("DTSTART:{}\nRRULE:{}", dtstart.format("%Y%m%dT%H%M%SZ"), to_rrule(rule_str)?);
let set: RRuleSet = rrule_str.parse()?;
set.into_iter().find(|dt| dt.date_naive() > after_date)
   .map(|dt| dt.date_naive())
   .ok_or(...)
```

### 7.3 `tm forecast`

Iterates all recurrence series (grouped by `recurrence_id`), finds the latest active
instance per series, then calls `next_due` repeatedly to generate dates within the
horizon. Results are sorted by date and filtered with the same `FilterSet` as `tm list`.

---

## 8. Filtering pipeline

`domain::filter::FilterSet` holds the parsed filter state:

```rust
pub struct FilterSet {
    pub required_tags: Vec<String>,    // +tag
    pub excluded_tags: Vec<String>,    // -tag
    pub project_prefix: Option<String>,
    pub context_override: Option<Vec<String>>,
    pub stage: Option<Stage>,
    pub include_future: bool,
    pub disable_implicit: bool,        // --all
}
```

`fn apply(tasks: Vec<Task>, filter: &FilterSet, state: &GlobalState) -> Vec<Task>`:

1. **Implicit gate** (skipped when `disable_implicit`):
   - Exclude tasks with `status != open`
   - Exclude tasks whose `start_date > today`
   - Exclude tasks that are effectively blocked (explicit `blocked_by` with any open
     blocker, or `parent_id` with any open sibling)
   - Exclude tasks with any unavailable `$resource` tag
   - Apply context filtering (see §3.1 of REQUIREMENTS.md)
2. **Explicit filters** (always applied):
   - `required_tags`: task must contain all
   - `excluded_tags`: task must contain none
   - `project_prefix`: `task.project.starts_with(prefix)`
   - `stage`: exact match
3. If `include_future`: also include tasks with `start_date > today` and synthetic
   recurrence instances generated by `recurrence::forecast_tasks`

The DB query pre-filters on `status`, `stage`, and `project` for efficiency; the Rust
layer handles the rest.

---

## 9. Natural-language date parsing

`domain::date_parse::parse_date(expr: &str, today: NaiveDate) -> Result<NaiveDate>`

Thin wrapper over `interim::parse_date_string`:

```rust
use interim::{parse_date_string, Dialect};
use chrono::Local;

pub fn parse_date(expr: &str, today: NaiveDate) -> Result<NaiveDate> {
    // Try ISO 8601 first (YYYY-MM-DD) to avoid ambiguity
    if let Ok(d) = NaiveDate::parse_from_str(expr, "%Y-%m-%d") {
        return Ok(d);
    }
    let base = today.and_hms_opt(0, 0, 0).unwrap().and_local_timezone(Local).unwrap();
    let dt = parse_date_string(expr, base, Dialect::Us)
        .map_err(|e| AppError::InvalidDate(expr.to_string(), e.to_string()))?;
    Ok(dt.date_naive())
}
```

Accepted by `--due` and `--start` in `tm add` and `tm edit`.

---

## 10. Integrations

### 10.1 Forgejo

`cli/commands/import_forgejo.rs` calls the Forgejo REST API using `reqwest` (blocking).

API base: read from `$XDG_CONFIG_HOME/task-manager/config.toml` as `forgejo.base_url`
and `forgejo.token`.

Endpoints used:
- `GET /api/v1/repos/{owner}/{repo}/issues?type=issues&state=open&limit=50&page=N`
- `GET /api/v1/repos/{owner}/{repo}/issues?type=issues&state=closed&limit=50&page=N`
- `PATCH /api/v1/repos/{owner}/{repo}/issues/{index}` with `{"state": "closed"}`
  (called from `tm done` when `task.forgejo_issue` is set)

Re-import matching: `task.forgejo_issue` URL is unique per issue; if a task with that URL
already exists it is updated (status only); otherwise a new task is created.
User-edited fields (title, notes, tags beyond issue labels, priority, due) are never
overwritten on re-import.

### 10.2 iCalendar

Uses the `icalendar` crate.

**Import** (`cli/commands/import_ical.rs`):
- Fetches URL via `reqwest` or reads file
- Parses with `icalendar::Calendar::from_str`
- Iterates `Component::Todo` entries
- Matches by `UID` property → `task.webcal_uid`
- Only reads `STATUS`; everything else is ignored

**Export** (`cli/commands/export_ical.rs`):
- Builds `icalendar::Calendar` with one `Todo` per task
- Maps: `title→SUMMARY`, `due→DUE`, `status→STATUS`, `uuid→UID`
- Priority: `high→1`, `medium→5`, `low→9` (iCalendar scale)
- Writes to `--output` file or stdout

---

## 11. Configuration

`~/.config/task-manager/config.toml` (XDG):

```toml
[forgejo]
base_url = "https://forgejo.victorsavu.eu"
token    = "..."

[scoring]
due_overdue_base   = 12.0
due_overdue_per_day = 0.3
due_week_base      = 6.0
due_week_per_day   = 0.8
due_month_base     = 3.0
due_month_per_day  = 0.1
priority_low       = 0.0
priority_medium    = 1.0
priority_high      = 2.0
project_low        = -0.5
project_medium     = 0.0
project_high       = 0.5
age_per_day        = 0.01
age_max            = 2.0

[forecast]
horizon_days = 90

[output]
next_count = 10   # default N for `tm next`
```

Config is loaded once at startup into a `Config` struct passed through `AppContext`.

---

## 12. Error handling

```rust
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("invalid date expression '{0}': {1}")]
    InvalidDate(String, String),      // exit 1

    #[error("task not found: {0}")]
    TaskNotFound(String),             // exit 1

    #[error("ambiguous task ID prefix '{0}': matches {1}")]
    AmbiguousId(String, usize),       // exit 1

    #[error("git conflict in files: {0:?}")]
    GitConflict(Vec<PathBuf>),        // exit 2

    #[error("forgejo API error: {0}")]
    ForgejoApi(String),               // exit 2

    #[error(transparent)]
    Io(#[from] std::io::Error),       // exit 2

    #[error(transparent)]
    Other(#[from] anyhow::Error),     // exit 2
}
```

`main` matches on the error variant to select the exit code (1 = user error, 2 = system
error). All error messages are printed to stderr.

Task IDs in CLI input accept any unambiguous prefix (minimum 4 hex characters). The DB
query uses `WHERE id LIKE '{prefix}%'`; if more than one row matches, `AmbiguousId` is
returned.

---

## 13. Testing strategy

| Layer | Approach |
|-------|----------|
| `domain::scoring` | Unit tests with fixed dates; test each factor independently |
| `domain::filter` | Unit tests: build `FilterSet` + `Vec<Task>`, assert filtered output |
| `domain::recurrence` | Unit tests: assert `next_due` for each supported pattern; table-driven |
| `domain::date_parse` | Unit tests: fixed "today", assert parsed date for common expressions |
| `storage::toml_store` | Round-trip tests: write task to temp dir, read back, assert equal |
| `storage::db` | Integration tests against in-memory SQLite (`:memory:`) |
| `storage::git` | Integration tests against `tempdir` git repo; assert commits created |
| CLI commands | Integration tests: set up temp git repo, invoke command handler, assert TOML files + DB state |
| Forgejo | Mock HTTP server (`mockito` crate); assert correct API calls |
| iCal | Parse known `.ics` fixtures; assert task fields; round-trip export |

No test mocks the file system for TOML — tests use real `tempdir` directories to catch
path and encoding issues.
