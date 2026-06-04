# Architecture

This document describes the internal design of `next`. Read `REQUIREMENTS.md` for the
"what"; this document covers the "how" and the "why".

---

## 1. Component overview

```
┌──────────────────────────────────────────────────────────────────┐
│                         next binary                              │
│  ┌──────────┐   ┌──────────────┐   ┌──────────────────────────┐ │
│  │   CLI    │──▶│    Domain    │──▶│        Storage           │ │
│  │ (clap)   │   │  (pure Rust) │   │  CachedStore (SQLite)    │ │
│  └──────────┘   └──────┬───────┘   │    └── TomlStore (TOML) │ │
│  ┌──────────┐   ┌──────▼───────┐   │    └── GitBackend (git2)│ │
│  │  Output  │◀──│   Filter /   │   └──────────────────────────┘ │
│  │text/JSON │   │   Scoring    │                                │
│  └──────────┘   └──────────────┘                                │
└──────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────┐
│     next-mcp binary  (feature = "mcp")  runs as UID 1000         │
│  ┌──────────────────┐  ┌─────────────┐  ┌──────────────────────┐│
│  │  HTTP server     │  │ MCP tools   │  │   Sync manager       ││
│  │  axum, 64 KB cap │─▶│ (same Store │  │ Semaphore(1) guards  ││
│  │  Bearer auth     │  │  & VcsBack) │  │ deferred/periodic    ││
│  └──────────────────┘  └─────────────┘  └──────────────────────┘│
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  git_init: HTTPS clone on first start; credentials stripped │ │
│  │  from logs; default git identity set in local repo config   │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

The domain layer is pure logic with no I/O. Storage calls into domain types but not
vice versa. Both the CLI and MCP layers wire them together via `AppContext`. All MCP
modules live in `src/mcp/` and are gated by the `mcp` Cargo feature.

---

## 2. Module layout

The project is a single crate named `next` with a library (`src/lib.rs`) and two
binaries: `src/main.rs` (CLI) and `src/mcp/main.rs` (MCP server, requires `--features mcp`).

```
next/                             # crate root (also git repo)
  Cargo.toml                      # [package] manifest; features: mcp (default = off)
  Containerfile                   # multi-stage build for next-mcp container image
  quadlets/
    next-mcp.container            # Podman Quadlet systemd unit file
  src/
    main.rs                       # `next` binary entry point
    mcp/
      main.rs                     # `next-mcp` binary entry point (requires mcp feature)
      mod.rs                      # library module root (pub re-exports for tests)
      config.rs                   # McpConfig: all config from env vars
      git_init.rs                 # clone_or_open(): HTTPS git clone on first start
      protocol.rs                 # JSON-RPC 2.0 + MCP types
      auth.rs                     # Bearer token middleware (MCP + webhook); constant-time comparison that does not leak token length
      sync_manager.rs             # do_sync(), SyncScheduler (Semaphore(1) + configurable deferred timer, default 30s), periodic sync
      server.rs                   # axum router (64 KB body limit), MCP dispatch, webhook handler
      tools/
        mod.rs                    # all_tools() registry + dispatch()
        tasks.rs                  # list_tasks, get_task, add_task, update_task, delete_task; validate_slug (allowlist: a-zA-Z0-9-_)
        state.rs                  # sync, get_state, set_context, set_resource, set_user_filter
        tags.rs                   # manage_tag
        data.rs                   # manage_task_data; validate_key (allowlist: a-zA-Z0-9-_, max 256 chars)
        view.rs                   # get_forecast
    lib.rs                        # library root; public re-exports
    config.rs                     # Config, BackendConfig, BackendKind
    error.rs                      # AppError, Result
    store.rs                      # Store + VcsBackend traits
    domain/                       # pure domain types (no I/O)
      mod.rs
      task.rs       state.rs      tag.rs          service.rs
      filter.rs     scoring.rs    date_parse.rs   recurrence.rs
    storage/                      # local TOML + SQLite + git backend
      mod.rs                      # open(), task_path(), tag_meta_path(), encode/decode_tag_path, state_path_for_repo(), plugins_path_for_repo()
      filenames.rs                # generate_filename(), task_path(), title_to_slug()
      lock.rs                     # FileLock: re-entrant cross-process advisory lock
      toml_store.rs               # TomlStore: source-of-truth TOML file I/O
      cached_store.rs             # CachedStore: wraps TomlStore with SQLite read cache
      git_backend.rs              # GitBackend: implements VcsBackend via git2
    plugin/                       # external plugin export hook (machine-local)
      registry.rs                 # PluginRegistry: plugins.toml store (subscriptions)
      notify.rs                   # TaskEvent + notify(): fire-and-forget plugin spawn
    forgejo/                      # next-forgejo binary (feature = "forgejo")
      main.rs config.rs issues.rs tasks.rs reconcile.rs hook.rs
    app_context.rs                # AppContext struct + ::new()
    log.rs                        # Logger: append-only next.log with rotation
    resolve.rs                    # fn resolve_task_id(store, id_str) -> Result<Uuid>
    cli/
      mod.rs                      # top-level Cli struct + Command enum (clap derive)
      filter.rs                   # FilterArgs -> FilterSet
      render.rs                   # text column / --json rendering
      recurrence_parse.rs         # parse_recurrence(): --recur-schedule/completion/snap → Recurrence
      commands/
        add.rs        cancel.rs   context.rs  data.rs
        delete.rs     done.rs     edit.rs     forecast.rs
        init.rs       list.rs     mod.rs      move_cmd.rs
        next_cmd.rs   open.rs     resource.rs show.rs
        start.rs      stop.rs     sync.rs     tree.rs
        tutorial.rs   user.rs
        plugin/       # mod.rs: next plugin register/watch/unwatch/unregister/list
        tag/
          mod.rs      # TagSubcommand dispatch + list()
          meta.rs     # describe, set-url, set-priority, set-no-time-urgency, show, clear-*
          data.rs     # tag data set/get/unset/list
  tests/
    common/mod.rs                 # shared test helpers (TestEnv, setup())
    test_add.rs   test_data.rs    test_done.rs   test_edit.rs
    test_init.rs  test_list.rs    test_open.rs   test_tag.rs   test_tree.rs
    cache_sync.rs locking.rs      migration.rs   sync.rs
    test_mcp.rs                   # in-process MCP HTTP integration tests (requires --features mcp)
    test_container.rs             # container integration tests (requires CONTAINER_TESTS=1)
```

---

## 3. Module responsibilities

### `next` — core library

**Domain types** (`next::domain`):

| Module | Contents |
|--------|----------|
| `task` | `Task`, `Status` (`Open`/`Started`/`Done`/`Cancelled`), `Priority`, `Recurrence`, `Snap` |
| `recurrence` | `fn next_occurrence(rrule, anchor, after)`, `fn apply_snap(date, snap)`, `fn spawn_next(task, today)` |
| `state` | `GlobalState` (active contexts, excluded contexts, active users, resource availability map) |
| `tag` | `TagKind` (Context / Resource / Freeform); `validate_tag` (allowlist: segments start with letter, contain `a-zA-Z0-9-_`, `/` separator allowed, `..` explicitly rejected); `validate_context_tag` (enforces `@` prefix); `validate_resource_tag` (enforces `#` prefix) |
| `filter` | `FilterSet`, `fn apply(tasks, filter, state) -> Vec<Task>` |
| `scoring` | `ScoredTask`, `ScoringWeights`, `fn score_and_sort(tasks, all_tasks, today, weights, tag_metas)` |
| `date_parse` | `fn parse_date(expr, today) -> Result<NaiveDate>` |
| `service` | `CreateTaskParams`, `EditTaskParams`, `create_task()`, `complete_task()`, `apply_edits()`, `validate_slug()`, `validate_url()` — shared business logic used by both CLI and MCP handlers |

Key `Task` fields: `id`, `title`, `status`, `priority`, `due`, `start`, `long_term`,
`slug`, `parent_id`, `assignee`, `tags`, `blocked_by`, `score_adjustment`, `description`,
`url`, `notes`, `data` (arbitrary JSON map; `data["time_log"]` accumulates start/stop events),
`recurrence`, `created_at`, `updated_at`.

Key `Task` methods: `is_open()` (Open only), `is_active()` (Open or Started), `mark_started()`,
`mark_stopped()`, `mark_done()`, `mark_cancelled()`.

**Storage traits** (`next::store`):

```rust
pub trait Store: Send + Sync {
    fn get_task(&self, id: Uuid) -> Result<Task>;
    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>>;
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>>;
    fn list_tasks(&self) -> Result<Vec<Task>>;
    fn save_task(&mut self, task: &Task) -> Result<()>;
    fn delete_task(&mut self, id: Uuid) -> Result<()>;
    fn get_state(&self) -> Result<GlobalState>;
    fn save_state(&mut self, state: &GlobalState) -> Result<()>;
    // Core tag metadata (stored in tags/<tag>.toml; committed to git)
    fn get_tag_meta(&self, tag: &str) -> Result<Option<TagMeta>>;
    fn set_tag_meta(&mut self, tag: &str, meta: TagMeta) -> Result<()>;
    fn delete_tag_meta(&mut self, tag: &str) -> Result<()>;
    fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>>;

    // Convenience wrappers implemented as default trait methods
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
    pub scoring: ScoringConfig,
    pub sync: SyncConfig,              // git_subprocess: use shell git for push/pull
    pub forecast_horizon_days: u32,    // default 90
    pub next_count: usize,             // default 10
    pub list_limit: Option<usize>,     // cap `next list` output; None = unlimited
    pub repository: Option<PathBuf>,   // default repo root (overridden by --repo)
    pub autosync: bool,                // sync after each mutation (overridden by --autosync)
}

pub struct BackendConfig {
    pub kind: BackendKind,             // Local (default and only supported value)
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

### `next::storage` — local TOML + SQLite cache + git backend

**`CachedStore`** is the `Store` implementation returned by `open()`. It wraps
`TomlStore` and maintains an SQLite database at `<repo>/.next.db`:

- **Reads** (`list_tasks`, `get_task`, `get_task_by_slug`, prefix lookups) query SQLite
  directly — no per-task TOML file reads.
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
    id   TEXT PRIMARY KEY,
    slug TEXT,
    data TEXT NOT NULL   -- full Task serialised as JSON
);
CREATE INDEX idx_tasks_slug ON tasks(slug);
```

**`TomlStore`** reads and writes one `.toml` file per task in `tasks/`.  Tag metadata
(`TagMeta`: description, URL, priority, `no_time_urgency`, arbitrary data) is stored as
individual TOML files under `tags/`. Tag names are encoded on disk: `@` → `__context__`,
`#` → `__resource__` (e.g. `@work` → `tags/__context__work.toml`,
`@home/kitchen` → `tags/__context__home/kitchen.toml`).
A one-time startup migration in `storage::open()` renames any existing unencoded paths.

**Contexts and resources are just tags.** `@context` and `#resource` tags are classified
by their prefix (`@` or `#`) but share the same `tags/` storage as freeform tags. There
are no separate metadata commands for contexts or resources — use `next tag describe`,
`next tag set-url`, `next tag set-priority`, etc. for all tag kinds.  The `next context`
and `next resource` commands only manage the machine-local active-context / resource-availability
state stored in `state.toml`; they do not touch tag metadata.

Machine-local state (active contexts, active users, resource availability) is stored at
`$XDG_STATE_HOME/task-manager/<fnv1a-hash-of-canonical-repo-path>/state.toml`.  This
path is computed by `next::storage::state_path_for_repo(root)` and is never committed to
git.  A separate advisory lock file co-located with `state.toml` (`.state.toml.lock`)
guards concurrent writes.

Two one-time migrations run on `TomlStore::open()`:
1. **State file migration**: if `<repo>/state.toml` exists and the XDG path does not,
   the file is moved to the XDG location.
2. **Tag description migration**: if `state.toml` contains a legacy `[tag_descriptions]`
   table, each entry is extracted to its own file under `tags/` and the table is removed.

Both migrations are idempotent (subsequent opens are no-ops).

**`GitBackend`** wraps `Mutex<git2::Repository>` to satisfy `Send + Sync`. Commit
messages follow the pattern `next: <verb> "<task title>"`.

### `next::cli` — command handlers

```
src/
  main.rs           # entry point: Init handled before AppContext; dispatches all others
  app_context.rs    # AppContext struct + ::new() (selects backend, opens store)
  log.rs            # Logger: append-only next.log with 1 MB rotation
  resolve.rs        # fn resolve_task_id(store, id_str) -> Result<Uuid>
  cli/
    mod.rs          # top-level Cli + Command enum (clap derive)
    filter.rs       # FilterArgs -> FilterSet conversion
    render.rs       # task list and detail rendering (text and --json)
    recurrence_parse.rs  # parse_recurrence(): shared by add.rs and edit.rs
    commands/
      init.rs       # next init — no AppContext needed; runs git init, creates tasks/
      tutorial.rs   # next tutorial — no AppContext needed; prints embedded TUTORIAL.md
      add.rs        cancel.rs   context.rs  data.rs
      delete.rs     done.rs     edit.rs     forecast.rs
      list.rs       mod.rs      move_cmd.rs next_cmd.rs
      open.rs       resource.rs show.rs     start.rs
      stop.rs       sync.rs     tree.rs     user.rs
      tag/
        mod.rs      # TagSubcommand dispatch + list()
        meta.rs     # describe, set-url, set-priority, set-no-time-urgency, show, clear-*
        data.rs     # tag data set/get/unset/list
```

---

## 4. Application context

Mutation command handlers receive `&mut AppContext`; read-only commands take `&AppContext`:

```rust
pub struct AppContext {
    pub config: Config,
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    pub repo_root: PathBuf,     // absolute path; used for log placement and task paths
    pub log: Logger,
}

impl AppContext {
    pub fn store(&self) -> &dyn Store { … }
    pub fn store_mut(&mut self) -> &mut dyn Store { … }
}
```

`AppContext::new()` opens the local backend:

- Walk up from CWD for `.git`; call `next::storage::open(root)` to get
  `(CachedStore, GitBackend)`; fail if no git repo is found.

Remote access is provided via MCP — connect with `claude mcp add --transport http https://next-mcp.victorsavu.eu`.

---

## 5. Command execution lifecycle

`next init` and `next tutorial` run before `AppContext` is constructed:

```
1. Parse CLI args (clap)
2. If command is Init → run init::run(args, cwd); exit
   If command is Tutorial → print embedded TUTORIAL.md; exit
3. AppContext::new(): locate repository root, select backend, open CachedStore
4. Execute command logic (reads from store; writes to store + vcs)
5. Task mutations (add/edit/start/stop/done/cancel/delete/move/tag/data): vcs.commit(changed_paths, message)
   State mutations (context/resource/user): write to XDG state file only; no commit
6. Render output (text or JSON to stdout)
7. If autosync enabled and command succeeded and is a mutation (`add`/`start`/`stop`/`done`/`cancel`/`edit`/`delete`/`move`/`tag`/`data`): run sync (pull + push)
8. On error: ctx.log.error(cmd_name, message); propagate to main
```

Read-only commands (list, show, forecast, tree) skip steps 5 and 7 and take `&AppContext` rather than `&mut AppContext`.

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

Task and tag-description mutations call `vcs.commit()` after writing TOML. Commit
messages follow the pattern `next: <verb> "<task title>"` — e.g. `next: add "Water plants"`.
State mutations (context, resource, user) write only to the XDG state file and do not
produce a git commit.

`pull` returns `PullResult::Clean` or `PullResult::Conflicts(Vec<PathBuf>)`. The sync
command aborts and prints conflicting file paths when conflicts are detected.

### 6.3 Concurrency and locking

Multiple `next` processes — CLI invocations, the long-running `next-mcp` server, and
external plugin processes — mutate the same repository in parallel. Several
mechanisms keep this safe:

1. **Re-entrant advisory lock** (`storage::FileLock`, `src/storage/lock.rs`). A generic
   advisory `flock(2)` over a given lock file, exclusive across processes and threads but
   **re-entrant within a single thread** so a transaction can hold it while the nested
   `save_task` / `commit` / `save_state` calls re-acquire it. A process-global registry
   maps each lock-file path to one in-process gate (owner thread + recursion depth)
   layered over the OS lock — plain `flock` is per open-file-description and would
   otherwise self-deadlock on the second acquire. Three *separate* instances are used: the
   **repo lock** (`<repo>/.next.lock`, shared by `TomlStore` task/tag writes and
   `GitBackend` commit/pull/push), the **state lock**, and the **plugins lock** (see below).

2. **Repository mutation transactions** (`AppContext::transaction`, built on
   `service::begin_mutation` / `end_mutation`). Every task or tag mutation holds the repo
   lock across the *entire* read → modify → write → commit sequence, so two processes
   cannot interleave and lose each other's updates. On entry the transaction reconciles
   the cache with the on-disk git HEAD (`Store::after_pull`) so the read reflects other
   processes' commits; on exit it records the new HEAD (`Store::note_head`) so the next
   transaction does not rebuild needlessly.

3. **State mutation transactions** (`AppContext::state_transaction`). Machine-local state
   (active contexts, excluded contexts, active users, resource availability) lives outside
   the git repository, so it has its own lock — `.state.toml.lock` next to the state file.
   Each `next context` / `resource` / `user` (and the matching MCP tool) holds this
   exclusive lock across its `get_state` → modify → `save_state`, closing the same
   lost-update window. No HEAD reconciliation, since state is never committed to git.

4. **SQLite WAL + busy_timeout** (`CachedStore::configure_connection`). `.next.db` is a
   single file shared by all processes; WAL lets readers and a writer proceed
   concurrently and the 5 s `busy_timeout` waits out transient locks instead of failing
   with `SQLITE_BUSY` (e.g. `next list` running during an MCP mutation). The WAL sidecars
   (`.next.db-wal` / `.next.db-shm`) are git-ignored.

5. **Plugin notification** (`plugin::notify`, see §6.4). Subscribed plugins are spawned
   only at the post-mutation chokepoints (`main.rs` after autosync; MCP `tools::dispatch`
   after the tool runs), i.e. **after the repo lock is released** — a plugin typically
   calls back into `next` and would otherwise deadlock. The plugins registry has its own
   `.plugins.toml.lock`, taken only there and during `next plugin` edits.

The three locks are independent files and never block one another. When a path takes more
than one, the ordering is always repo-lock-before-state/plugins-lock, never the reverse, so
they cannot deadlock. `tests/locking.rs` covers lost-update prevention for task, state, and
plugin-subscription edits, slug-conflict races, and pull/commit coordination; re-entrant
lock unit tests live in `src/storage/lock.rs`.

### 6.4 Plugins (export hook)

External plugin binaries subscribe to individual tasks and are notified when those tasks
change. The registry (`plugin::registry`) is machine-local — `plugins.toml` in the per-repo
state dir (`storage::plugins_path_for_repo`), never committed — each entry being
`{ name, command: argv, tasks: [uuid] }`, managed by the `next plugin` subcommands.

Each task-mutating handler records a `(verb, task_id)` `TaskEvent` on `AppContext` after its
transaction returns. The CLI (`main.rs`) and MCP (`tools::dispatch`) chokepoints drain the
buffer and call `plugin::notify`, which spawns every subscribed plugin's command
fire-and-forget (event JSON on stdin + `NEXT_PLUGIN_EVENT`/`NEXT_REPO`/`NEXT_PLUGIN_ORIGIN`
env). The origin plugin is skipped (loop guard via `NEXT_PLUGIN_ORIGIN`), the long-lived
server reaps children on a helper thread, and `delete` events prune the subscription. See
REQUIREMENTS.md §10 for the full contract.

---

## 7. Logging

```
src/log.rs
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
   - Exclude tasks where `!is_active()` — i.e. `done` and `cancelled` are hidden; `open` and `started` pass
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

`src/resolve.rs`:

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
  + tag_factor(task.tags, tag_metas)
  + tag_factor(parent.tags, tag_metas) // 0.0 when no parent
  + started_factor(task.status)
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

**`age_factor`**: `min(age_days × 0.01, 2.0)` — returns `0.0` when `long_term = true`,
`start > today`, or any of the task's tags has `no_time_urgency = true`.

**`tag_factor`**: sum of the priority offset for each tag that has an explicit `priority`
set in its `TagMeta`. Tags with no metadata or no priority set contribute `0.0`.
Applied once for the task's own tags and once for the parent's tags (if any).
Default offsets: `tag_low = −1.0`, `tag_medium = 0.0`, `tag_high = +1.0`.
A low-priority tag exactly cancels a medium-priority task's base score (−1.0 + 1.0 = 0).

**`no_time_urgency`**: when any of the task's tags has `TagMeta::no_time_urgency = true`,
both `due_factor` and `age_factor` are forced to `0.0`. Set with
`next tag set-no-time-urgency <tag>`.

**`started_factor`**: `+4.0` (configurable) when `status == Started`; `0.0` otherwise.

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
repository            = "/home/alice/tasks"  # use next from any directory
autosync              = false                # sync after each mutation (--autosync to override)
list_limit            = 20                   # cap `next list` output; absent = unlimited
forecast_horizon_days = 90
next_count            = 10                   # tasks shown by `next next`

[backend]
kind = "local"

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
tag_low             = -1.0   # offset when a task (or its parent) has a low-priority tag
tag_medium          =  0.0   # neutral — tags without explicit priority contribute nothing
tag_high            =  1.0   # offset when a task (or its parent) has a high-priority tag
started_bonus       =  4.0   # flat bonus added when status == started
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
| `domain::scoring` | `src/domain/scoring.rs` | Unit tests with fixed dates; each factor tested independently |
| `domain::filter` | `src/domain/filter.rs` | Unit tests: build `FilterSet` + `Vec<Task>`, assert filtered output |
| `domain::date_parse` | `src/domain/date_parse.rs` | Unit tests: fixed "today", assert parsed date for common expressions |
| `TomlStore` | `src/storage/toml_store.rs` | Round-trip tests: write task to `tempdir`, read back, assert equal fields; migration unit tests: write legacy `state.toml`, call `TomlStore::open()`, assert per-tag files created and `state.toml` cleaned |
| `GitBackend` | `src/storage/git_backend.rs` | Integration tests against `tempdir` git repo; assert commits and HEAD |
| `CachedStore` | `src/storage/cached_store.rs` | Unit tests: save/retrieve/delete/rebuild within a `tempdir` git repo |
| Cache sync | `tests/cache_sync.rs` | Integration tests: write-through consistency (SQLite ↔ TOML), git pull propagation (HEAD change triggers rebuild), cache-reuse (same HEAD = no rebuild) |
| Migration | `tests/migration.rs` | Integration tests: write legacy `state.toml` with `[tag_descriptions]`, call `next::storage::open()`, assert per-tag files, state cleanup, idempotency, and persistence across reopens |
| File locking | `tests/locking.rs` | Concurrency tests: multiple threads open independent `TomlStore`/`GitBackend` instances (simulating separate processes) and assert no data loss or corruption, including transactional lost-update prevention (N processes each add a distinct tag to one task; all must survive). Re-entrant lock unit tests live in `src/storage/lock.rs` |
| CLI commands | `tests/test_*.rs` | Integration tests: construct `AppContext` directly in a `tempdir` git repo; call `run()` functions; assert store state |
| MCP unit tests | `src/mcp/tools/*.rs` | Unit tests per tool module using real `AppContext` in a `tempdir` git repo (requires `--features mcp`) |
| MCP integration tests | `tests/test_mcp.rs` | Start a real HTTP server on `127.0.0.1:0` in `#[tokio::test]`; test all 13 tools, auth, webhook, and autosync (requires `--features mcp`) |
| Container tests | `tests/test_container.rs` | Start the real container image via `testcontainers` (Podman); opt-in with `CONTAINER_TESTS=1 DOCKER_HOST=unix:///…/podman.sock`; covers git clone, auth, sync push, deferred timer, webhook, and idempotent restart |

---

## 15. Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing |
| `git2` | git commit / pull / push |
| `toml` | TOML file serialisation |
| `rusqlite` | SQLite read cache |
| `fs4` | cross-platform advisory file locking (`flock(2)`) |
| `serde`, `serde_json` | serialisation (domain types, `--json` output) |
| `chrono` | dates in domain types and scoring |
| `interim` | `date_parse` module (natural-language date expressions) |
| `uuid` | `Task::id` |
| `dirs` | XDG base directory resolution |
| `anyhow`, `thiserror` | error propagation |

MCP-only dependencies (feature = `"mcp"`):

| Crate | Purpose |
|-------|---------|
| `tokio` | async runtime for `next-mcp` |
| `axum` | HTTP server, middleware, `DefaultBodyLimit` |
