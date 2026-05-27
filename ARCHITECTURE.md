# Architecture

This document describes the internal design of `next`. Read `REQUIREMENTS.md` for the
"what"; this document covers the "how" and the "why".

---

## 1. Component overview

```
┌──────────────────────────────────────────────────────────────────┐
│                         next binary                              │
│                                                                  │
│  ┌──────────┐   ┌──────────────┐   ┌──────────────────────────┐ │
│  │   CLI    │──▶│    Domain    │──▶│        Storage           │ │
│  │ (clap)   │   │  (pure Rust) │   │  CachedStore (SQLite)    │ │
│  └──────────┘   └──────┬───────┘   │    └── TomlStore (TOML) │ │
│                        │           │    └── GitBackend (git2) │ │
│  ┌──────────┐   ┌──────▼───────┐   └──────────────────────────┘ │
│  │  Output  │◀──│   Filter /   │                                │
│  │text/JSON │   │   Scoring    │                                │
│  └──────────┘   └──────────────┘                                │
│                                                                  │
│  ┌──────────────────────────────────────────────────────┐       │
│  │                  Integrations (planned)              │       │
│  │    Forgejo (HTTP/REST)   │   iCalendar (ical)        │       │
│  └──────────────────────────────────────────────────────┘       │
└──────────────────────────────────────────────────────────────────┘
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
| `task` | `Task`, `Status`, `Priority`, `Recurrence` |
| `state` | `GlobalState` (active contexts, active users, resource availability map) |
| `tag` | `TagKind` (Context / Resource / Freeform); tag parsing helpers |
| `filter` | `FilterSet`, `fn apply(tasks, filter, state) -> Vec<Task>` |
| `scoring` | `ScoredTask`, `ScoringWeights`, `fn score_and_sort(tasks, all_tasks, today, weights)` |
| `date_parse` | `fn parse_date(expr, today) -> Result<NaiveDate>` |

Key `Task` fields: `id`, `title`, `status`, `priority`, `due`, `start`, `long_term`,
`slug`, `parent_id`, `assignee`, `tags`, `blocked_by`, `score_adjustment`, `description`,
`url`, `notes`, `data` (arbitrary JSON map), `recurrence`, `created_at`, `updated_at`.

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
    fn get_tag_description(&self, tag: &str) -> Result<Option<String>>;
    fn set_tag_description(&mut self, tag: &str, desc: &str) -> Result<()>;
    fn delete_tag_description(&mut self, tag: &str) -> Result<()>;
    fn list_tag_descriptions(&self) -> Result<HashMap<String, String>>;
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
    pub repository: Option<PathBuf>,  // default repository path (overridden by --repo)
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

### `next-storage` — local TOML + SQLite cache + git backend

```
crates/next-storage/src/
  lib.rs            # pub fn open(root: PathBuf) -> Result<(CachedStore, GitBackend)>
                    # pub fn task_path(root, task) -> PathBuf
  cached_store.rs   # CachedStore: wraps TomlStore with an SQLite read cache
  toml_store.rs     # TomlStore: source-of-truth TOML file I/O
  git_backend.rs    # GitBackend: implements VcsBackend (via git2)
```

**`CachedStore`** is the `Store` implementation returned by `open()`. It wraps
`TomlStore` and maintains an SQLite database at `<repo>/.next.db`:

- **Reads** (`list_tasks`, `get_task`, `get_task_by_slug`, prefix/forgejo/webcal
  lookups) query SQLite directly — no per-task TOML file reads.
- **Writes** (`save_task`, `delete_task`, `save_state`) write to TOML first
  (authoritative), then update the SQLite cache in-place.
- **Cache invalidation**: on `open()`, `CachedStore` compares the stored git HEAD hash
  against the current HEAD (from `GitBackend::head_hash()`). A mismatch triggers a full
  rebuild: all TOML files are read via `TomlStore::list_tasks()` and the SQLite tables
  are repopulated. The new HEAD hash is stored in the `meta` table.

SQLite schema:

```sql
CREATE TABLE meta  (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE tasks (
    id            TEXT PRIMARY KEY,
    slug          TEXT,
    forgejo_issue TEXT,
    webcal_uid    TEXT,
    data          TEXT NOT NULL   -- full Task serialised as JSON
);
CREATE INDEX idx_tasks_slug    ON tasks(slug);
CREATE INDEX idx_tasks_forgejo ON tasks(forgejo_issue);
CREATE INDEX idx_tasks_webcal  ON tasks(webcal_uid);
```

**`TomlStore`** reads and writes one `.toml` file per task in `tasks/`.  The global state
is stored in `state.toml` at the repository root.  Tag descriptions are stored as
individual TOML files under `tags/`: the tag string maps directly to a path (`@work` →
`tags/@work.toml`, `@home/kitchen` → `tags/@home/kitchen.toml`).

A one-time migration runs on `TomlStore::open()`: if `state.toml` contains a legacy
`[tag_descriptions]` table, each entry is extracted to its own file under `tags/` and
the table is removed from `state.toml`.  The migration is idempotent (subsequent opens
are no-ops).

`next_storage::tag_description_path(root, tag)` returns the canonical path for a tag's
description file and is used by CLI commands to pass the correct path to `vcs.commit()`.

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
  main.rs           # entry point: Init handled before AppContext; dispatches all others
  app_context.rs    # AppContext struct + ::new() (selects backend, opens store)
  lib.rs            # re-exports AppContext; makes commands importable from tests
  log.rs            # Logger: append-only next.log with 1 MB rotation
  resolve.rs        # fn resolve_task_id(store, id_str) -> Result<Uuid>
  cli/
    mod.rs          # top-level Cli + Command enum (clap derive)
    filter.rs       # FilterArgs -> FilterSet conversion
    render.rs       # task list and detail rendering (text and --json)
    commands/
      init.rs       # next init — no AppContext needed; runs git init, creates tasks/
      add.rs        cancel.rs   context.rs  data.rs
      delete.rs     done.rs     edit.rs     export.rs
      forecast.rs   import.rs   list.rs     mod.rs
      move_cmd.rs   next_cmd.rs open.rs     resource.rs
      show.rs       sync.rs     tag.rs      tree.rs
      user.rs
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
  `(CachedStore, GitBackend)`; fail if no git repo is found.
- **Remote**: use CWD as `repo_root`; construct `RemoteStore` + `RemoteVcs` from
  the configured URL and token.

---

## 5. Command execution lifecycle

`next init` is the only command that runs before `AppContext` is constructed:

```
1. Parse CLI args (clap)
2. If command is Init → run init::run(args, cwd); exit
3. AppContext::new(): locate repository root, select backend, open CachedStore
4. Execute command logic (reads from store; writes to store + vcs)
5. If mutation: vcs.commit(changed_paths, message)
6. Render output (text or JSON to stdout)
7. On error: ctx.log.error(cmd_name, message); propagate to main
```

Read-only commands (list, show, forecast) skip step 5.

---

## 6. Storage layer

`next_storage::open(root)` returns `(CachedStore, GitBackend)`. `CachedStore` satisfies
the `Store` trait; callers box it as `Box<dyn Store>` inside `AppContext`.

### 6.1 TOML file conventions

- One `.toml` file per task in a flat `tasks/` directory
- Filename: `<slug>.toml` if the task has a user slug; otherwise
  `<title-slug>-<first-8-uuid-hex>.toml`
- Title slug: lowercase, spaces → `-`, strip non-alphanumeric except `-`
- Optional fields are omitted rather than written as empty strings or nulls
- The `[recurrence]` table is only present when the task recurs
- The `[data]` table is only present when at least one key has been set

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
    pub include_future: bool,
    pub disable_implicit: bool,            // --all
}
```

`fn apply(tasks: Vec<Task>, filter: &FilterSet, state: &GlobalState, today: NaiveDate) -> Vec<Task>`:

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
  + project_factor(parent.priority)   // 0.0 when no parent
  + age_factor(task, today)
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

**`project_factor`** (parent task priority): `low` → −0.5, `medium` → 0.0, `high` → +0.5

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
project_low         = -0.5
project_medium      = 0.0
project_high        = 0.5
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

| Layer | Location | Approach |
|-------|----------|----------|
| `domain::scoring` | `crates/next/src/domain/scoring.rs` | Unit tests with fixed dates; each factor tested independently |
| `domain::filter` | `crates/next/src/domain/filter.rs` | Unit tests: build `FilterSet` + `Vec<Task>`, assert filtered output |
| `domain::date_parse` | `crates/next/src/domain/date_parse.rs` | Unit tests: fixed "today", assert parsed date for common expressions |
| `TomlStore` | `crates/next-storage/src/toml_store.rs` | Round-trip tests: write task to `tempdir`, read back, assert equal fields; migration unit tests: write legacy `state.toml`, call `TomlStore::open()`, assert per-tag files created and `state.toml` cleaned |
| `GitBackend` | `crates/next-storage/src/git_backend.rs` | Integration tests against `tempdir` git repo; assert commits and HEAD |
| `CachedStore` | `crates/next-storage/src/cached_store.rs` | Unit tests: save/retrieve/delete/rebuild within a `tempdir` git repo |
| Cache sync | `crates/next-storage/tests/cache_sync.rs` | Integration tests: write-through consistency (SQLite ↔ TOML), git pull propagation (HEAD change triggers rebuild), cache-reuse (same HEAD = no rebuild) |
| Migration | `crates/next-storage/tests/migration.rs` | Integration tests: write legacy `state.toml` with `[tag_descriptions]`, call `next_storage::open()`, assert per-tag files, state cleanup, idempotency, and persistence across reopens |
| File locking | `crates/next-storage/tests/locking.rs` | Concurrency tests: multiple threads open independent `TomlStore`/`GitBackend` instances (simulating separate processes) and assert no data loss or corruption |
| CLI commands | `crates/next-cli/tests/` | Integration tests: construct `AppContext` directly in a `tempdir` git repo; call `run()` functions; assert store state |
