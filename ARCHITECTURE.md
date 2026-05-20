# Architecture

This document describes the internal design of `next`. Read `REQUIREMENTS.md` for the
"what"; this document covers the "how" and the "why".

---

## 1. Component overview

```
┌─────────────────────────────────────────────────────────────┐
│                       next binary                           │
│                                                             │
│  ┌──────────┐   ┌──────────────┐   ┌─────────────────────┐ │
│  │   CLI    │──▶│    Domain    │──▶│      Storage        │ │
│  │ (clap)   │   │  (pure Rust) │   │ TOML files + git2   │ │
│  └──────────┘   └──────┬───────┘   └─────────────────────┘ │
│                        │                                    │
│  ┌──────────┐   ┌──────▼───────┐                           │
│  │  Output  │◀──│   Filter /   │                           │
│  │text/JSON │   │   Scoring    │                           │
│  └──────────┘   └──────────────┘                           │
│                                                             │
│  ┌────────────────────────────────────────────────────┐    │
│  │                Integrations (planned)              │    │
│  │   Forgejo (HTTP/REST)   │   iCalendar (ical)       │    │
│  └────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

The domain layer is pure logic with no I/O. Storage calls into domain types but not
vice versa. The CLI layer wires them together via `AppContext`.

---

## 2. Workspace layout

```
next/                             # workspace root
  Cargo.toml                      # [workspace] manifest
  crates/
    next/                         # library: domain types + storage traits + config
    next-storage/                 # library: TOML + git2 implementation
    next-remote-storage/          # library: stub HTTP backend (not yet implemented)
    next-cli/                     # binary: clap CLI wrapper
```

### Dependency graph

```
next-cli
  ├── next                (domain types, traits, config)
  ├── next-storage        (local Store + VcsBackend)
  └── next-remote-storage (stub remote Store + VcsBackend)

next-storage
  └── next

next-remote-storage
  └── next

next
  └── (no internal workspace deps)
```

---

## 3. Crate responsibilities

### `next` — core library

**Domain types** (`next::domain`):

| Module | Contents |
|--------|----------|
| `task` | `Task`, `Status`, `Stage`, `Priority`, `Recurrence` |
| `state` | `GlobalState` (active contexts, active users, resource map) |
| `tag` | `Tag`, `TagKind` (Context / Resource / Freeform) |
| `filter` | `FilterSet`, `fn apply(tasks, filter, state) -> Vec<Task>` |
| `scoring` | `ScoredTask`, `ScoringWeights`, `fn score_and_sort(tasks, state, weights)` |
| `date_parse` | `fn parse_date(expr, today) -> Result<NaiveDate>` |

**Storage traits** (`next::store`):

```rust
pub trait Store: Send + Sync {
    fn get_task(&self, id: Uuid) -> Result<Task>;
    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>>;
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>>;
    fn list_tasks(&self) -> Result<Vec<Task>>;
    fn save_task(&mut self, task: &Task) -> Result<()>;
    fn delete_task(&mut self, id: Uuid) -> Result<()>;
    fn get_task_by_forgejo_issue(&self, url: &str) -> Result<Option<Task>>;
    fn get_task_by_webcal_uid(&self, uid: &str) -> Result<Option<Task>>;
    fn get_state(&self) -> Result<GlobalState>;
    fn save_state(&mut self, state: &GlobalState) -> Result<()>;
}

pub trait VcsBackend: Send + Sync {
    fn commit(&self, paths: &[PathBuf], message: &str) -> Result<()>;
    fn pull(&self) -> Result<PullResult>;   // PullResult: Clean | Conflicts(Vec<PathBuf>)
    fn push(&self) -> Result<()>;
    fn head_hash(&self) -> Result<String>;
}
```

**Configuration** (`next::config`):

```rust
pub struct Config {
    pub backend: BackendConfig,
    pub scoring: ScoringWeights,
}

pub struct BackendConfig {
    pub kind: BackendKind,          // Local (default) | Remote
    pub remote: Option<RemoteBackendConfig>,
}

pub struct RemoteBackendConfig {
    pub url: String,
    pub token: Option<String>,
}
```

**Error type** (`next::error`):

```rust
pub enum AppError {
    TaskNotFound(String),
    AmbiguousId(String, usize),
    GitConflict(Vec<PathBuf>),
    Io(std::io::Error),
    Other(String),
}
```

### `next-storage` — local TOML + git backend

```
crates/next-storage/src/
  lib.rs            # pub fn open(root: PathBuf) -> Result<(TomlStore, GitBackend)>
                    # pub fn task_path(root, task) -> PathBuf
  toml_store.rs     # TomlStore: implements Store
  git_backend.rs    # GitBackend: implements VcsBackend (via git2)
```

**`TomlStore`** reads and writes one `.toml` file per task in `tasks/`. The state is
stored in `state.toml` at the repository root. No SQLite; all reads scan the `tasks/`
directory.

**`GitBackend`** wraps `Mutex<git2::Repository>` to satisfy `Send + Sync`. Commit
messages follow the pattern `next: <verb> "<task title>"`.

### `next-remote-storage` — stub HTTP backend

```
crates/next-remote-storage/src/
  lib.rs            # re-exports RemoteStore, RemoteVcs
  remote_store.rs   # RemoteStore: all Store methods return AppError::Other("not yet implemented")
  remote_vcs.rs     # RemoteVcs: commit/push are no-ops; pull returns Clean; head_hash returns "remote"
```

Construction never fails; errors are returned lazily. The binary starts up cleanly even
before any server is reachable.

### `next-cli` — binary

```
crates/next-cli/src/
  main.rs           # entry point: build AppContext, dispatch command, log errors
  log.rs            # Logger: append-only next.log with 1 MB rotation
  resolve.rs        # fn resolve_task_id(store, id_str) -> Result<Uuid>
  cli/
    mod.rs          # top-level Cli + Command enum (clap derive)
    filter.rs       # FilterArgs -> FilterSet conversion
    render.rs       # task list and detail rendering (text and --json)
    commands/
      add.rs        cancel.rs   context.rs  delete.rs
      done.rs       edit.rs     export.rs   forecast.rs
      import.rs     list.rs     mod.rs      move_cmd.rs
      next_cmd.rs   project.rs  resource.rs review.rs
      show.rs       sync.rs     user.rs
```

---

## 4. Application context

Every command handler receives `&mut AppContext`:

```rust
pub struct AppContext {
    pub config: Config,
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    pub repo_root: PathBuf,     // absolute path; used for log placement and task paths
    pub log: Logger,
}
```

`AppContext::new()` selects the backend based on `config.backend.kind`:

- **Local**: walk up from CWD for `.git`; call `next_storage::open(root)` to get
  `(TomlStore, GitBackend)`; fail if no git repo is found.
- **Remote**: use CWD as `repo_root`; construct `RemoteStore` + `RemoteVcs` from
  the configured URL and token.

---

## 5. Command execution lifecycle

Every command follows this sequence:

```
1. Parse CLI args (clap)
2. AppContext::new(): locate repository root, select backend, open store
3. Execute command logic (reads from store; writes to store + vcs)
4. If mutation: vcs.commit(changed_paths, message)
5. Render output (text or JSON to stdout)
6. On error: ctx.log.error(cmd_name, message); propagate to main
```

Read-only commands (list, show, forecast) skip step 4.

---

## 6. Storage layer

### 6.1 TOML file conventions

- One `.toml` file per task in a flat `tasks/` directory
- Filename: `<slug>.toml` if the task has a user slug; otherwise
  `<title-slug>-<first-8-uuid-hex>.toml`
- Title slug: lowercase, spaces → `-`, strip non-alphanumeric except `-`
- Optional fields are omitted rather than written as empty strings or nulls
- The `[recurrence]` table is only present when the task recurs

### 6.2 Git operations

`GitBackend` wraps `git2`:

```rust
fn commit(&self, paths: &[PathBuf], message: &str) -> Result<()>
fn pull(&self) -> Result<PullResult>
fn push(&self) -> Result<()>
fn head_hash(&self) -> Result<String>
```

All mutations call `vcs.commit()` after writing TOML. Commit messages follow the pattern:
`next: <verb> "<task title>"` — e.g. `next: add "Water plants"`.

`pull` returns `PullResult::Clean` or `PullResult::Conflicts(Vec<PathBuf>)`. The sync
command aborts and prints conflicting file paths when conflicts are detected.

---

## 7. Logging

```
crates/next-cli/src/log.rs
```

`Logger` writes an append-only plaintext log at `<repo_root>/next.log`:

```
2026-05-17T12:00:00Z INFO  [add] added "Water plants" (a1b2c3d4)
2026-05-17T12:01:00Z ERROR [done] task not found: "xyz"
```

- Format: `{timestamp} {LEVEL} [{command}] {message}\n`
- Rotation: when `next.log` exceeds 1 MB, it is renamed to `next.log.1` before the next
  write. Only one backup is kept.
- Write failures are silently swallowed — logging never aborts a command.
- Mutation commands log success silently (no terminal output on success). Display commands
  write to stdout only.

---

## 8. Filtering pipeline

`domain::filter::FilterSet` holds the parsed filter state:

```rust
pub struct FilterSet {
    pub required_tags: Vec<String>,        // +tag
    pub excluded_tags: Vec<String>,        // -tag
    pub context_override: Option<Vec<String>>,
    pub user_override: Option<Vec<String>>,
    pub stage: Option<Stage>,
    pub include_future: bool,
    pub disable_implicit: bool,            // --all
}
```

`fn apply(tasks: Vec<Task>, filter: &FilterSet, state: &GlobalState) -> Vec<Task>`:

1. **Implicit gate** (skipped when `disable_implicit`):
   - Exclude tasks with `status != open`
   - Exclude tasks whose `start` date is in the future
   - Exclude tasks that are explicitly blocked (open entry in `blocked_by`)
   - Exclude parent tasks that have any open direct child
   - Exclude tasks with any unavailable `#resource` tag
   - Apply context filtering (tasks with no `@` tag always pass)
   - Apply user filtering (tasks with no `assignee` always pass)
2. **Explicit filters** (always applied):
   - `required_tags`: task must contain all
   - `excluded_tags`: task must contain none
   - `stage`: exact match
3. `include_future`: also include tasks with `start` date in the future

### User filter semantics

Context filtering and user filtering have different visibility rules for untagged tasks:

| Filter | Untagged task behaviour |
|--------|------------------------|
| Context | Always visible (tasks with no `@` tag are context-neutral) |
| User | Always visible (unassigned tasks belong to the shared backlog) |

This means `active_users = ["alice"]` hides tasks assigned to `bob` but never hides
tasks with no `assignee`.

---

## 9. ID resolution

`crates/next-cli/src/resolve.rs`:

```rust
pub fn resolve_task_id(store: &dyn Store, id_str: &str) -> anyhow::Result<Uuid>
```

Resolution order:
1. Full UUID — `Uuid::parse_str`
2. Slug — `store.get_task_by_slug`
3. UUID prefix (4+ chars) — `store.find_tasks_by_prefix`; error if > 1 match

---

## 10. Scoring

Scores are computed at query time (not stored) by `domain::scoring::score_and_sort`.

```
score(task) =
    due_factor(task.due, today)
  + priority_factor(task.priority)
  + age_factor(task.created_at, today, task.long_term, task.start)
  + task.score_adjustment
```

### Default weights

**`due_factor`**:

| Condition | Value |
|-----------|-------|
| No due date | `0.0` |
| Overdue by N days | `12.0 + N × 0.3` |
| Due today | `12.0` |
| Due in 1–7 days | `6.0 + (7 − days) × 0.8` |
| Due in 8–30 days | `3.0 + (30 − days) × 0.1` |
| Due in 31+ days | `max(0.0, 2.0 − days × 0.01)` |

**`priority_factor`**: `low` → 0.0, `medium` → 1.0, `high` → 2.0

**`age_factor`**: `min(age_days × 0.01, 2.0)` — returns `0.0` when `long_term = true`
or `start > today`.

---

## 11. Natural-language date parsing

`domain::date_parse::parse_date(expr: &str, today: NaiveDate) -> Result<NaiveDate>`

Thin wrapper over `interim::parse_date_string`:

1. Try ISO 8601 (`YYYY-MM-DD`) first to avoid ambiguity
2. Convert `today: NaiveDate` to `DateTime<Local>` (midnight) — required by interim's API
3. Call `interim::parse_date_string(expr, dt, Dialect::Uk)`
4. Extract `.date_naive()` from result

Accepted by `--due` and `--start` in `next add` and `next edit`.

---

## 12. Configuration

`$XDG_CONFIG_HOME/task-manager/config.toml` (loaded once at startup; falls back to
defaults when absent):

```toml
[backend]
kind = "local"   # "local" | "remote"

[backend.remote]
url   = "https://tasks.example.com"
token = "my-bearer-token"

[scoring]
due_overdue_base    = 12.0
due_overdue_per_day = 0.3
due_week_base       = 6.0
due_week_per_day    = 0.8
due_month_base      = 3.0
due_month_per_day   = 0.1
priority_low        = 0.0
priority_medium     = 1.0
priority_high       = 2.0
age_per_day         = 0.01
age_max             = 2.0
```

---

## 13. Error handling

```rust
pub enum AppError {
    #[error("task not found: {0}")]
    TaskNotFound(String),              // exit 1

    #[error("ambiguous task ID prefix '{0}': {1} matches")]
    AmbiguousId(String, usize),        // exit 1

    #[error("git conflict in files: {0:?}")]
    GitConflict(Vec<PathBuf>),         // exit 2

    #[error(transparent)]
    Io(#[from] std::io::Error),        // exit 2

    #[error("{0}")]
    Other(String),                     // exit 2
}
```

All error messages are printed to stderr. `main` logs the error via `Logger::error` and
propagates the `anyhow::Error` to produce a non-zero exit code.

---

## 14. Testing strategy

| Layer | Approach |
|-------|----------|
| `domain::scoring` | Unit tests with fixed dates; each factor tested independently |
| `domain::filter` | Unit tests: build `FilterSet` + `Vec<Task>`, assert filtered output; covers user filter, context filter, resource filter |
| `domain::date_parse` | Unit tests: fixed "today", assert parsed date for common expressions |
| `next-storage` | Round-trip tests: write task to `tempdir`, read back, assert equal fields |
| `next-storage` git | Integration tests against `tempdir` git repo; assert commits created |
