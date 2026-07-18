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
│  │  git_init: partial clone (blob:none) on first start, full-  │ │
│  │  clone fallback; credentials stripped                       │ │
│  │  from logs; default git identity set in local repo config   │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

The domain layer is pure logic with no I/O. Storage calls into domain types but not
vice versa. The runtime handle that ties the store, git backend, and scoring together is
`TaskRepository` (`src/core/task_repository.rs`); the MCP server and the Forgejo plugin use
it directly, while the CLI wraps it in `AppContext` to add `config.toml` handling (CLI-only).
All MCP modules live in `src/mcp/` and are gated by the `mcp` Cargo feature.

---

## 2. Module layout

The project is a single crate named `next` with a library (`src/lib.rs`) and four
feature-gated binaries: `src/cli/main.rs` (CLI, requires `cli` — on by default),
`src/mcp/main.rs` (MCP server, requires `mcp`), `src/forgejo/main.rs`
(`next-forgejo`, requires `forgejo`), and `src/tui/main.rs` (`next-tui`, requires `tui`).
With **no features** the crate is just the `core`
module — the task store, git backend, domain types, scoring/service/recurrence, the
plugin registry, and `TaskRepository` — which other crates can link without the CLI or
`clap`. The CLI's `AppContext` (config-file handling) and the feature modules sit on top.
See §2.1.

```
next/                             # crate root (also git repo)
  Cargo.toml                      # features: cli (default), mcp, forgejo, tui; clap/tracing-subscriber optional
  Containerfile                   # multi-stage build for next-mcp container image
  quadlets/
    next-mcp.container            # Podman Quadlet systemd unit file
    next-mcp.env.example          # environment-file template
    next-mcp.config.toml.example  # TOML config-file template (next-config volume)
  src/
    lib.rs                        # `pub mod core` + feature-gated cli/mcp/forgejo/tui; small type prelude
    core/                         # THE CORE LIBRARY — compiled with no features
      error.rs                    # TaskError, Result
      config.rs                   # Config + SyncConfig (config.toml schema; machine-local, no scoring)
      store.rs                    # Store + VcsBackend traits; TaskQuery, Page, paginate()
      resolve.rs                  # resolve_task_id(store, id_str) -> Result<Uuid>
      task_repository.rs          # TaskRepository: store + vcs + repo_root + scoring + transactions + plugin events
      bootstrap.rs                # config-file parsing + repo resolution + opening (shared by cli/tui)
      sync.rs                     # sync(): pull -> cache-reconcile -> auto-archive -> push; pull_if_stale()
      sync_state.rs               # machine-local [sync] state: last_pull, last_archive, per-plugin last_sync
      value.rs                    # parse_value(): task data value parsing
      filter_args.rs              # FilterArgs -> FilterSet (filter-token parsing)
      scoring.rs                  # ScoredTask, ScoringConfig, TaskDates, score_and_sort()
      service.rs                  # create_task/complete_task/apply_edits; begin/end_mutation
      recurrence.rs               # next_occurrence(), apply_snap(), spawn_next(), parse_snap()
      forecast.rs                 # forecast projection shared by cli/mcp/tui
      listing.rs                  # load_candidates() (status pushdown), extend_with_parents()
      archiver.rs                 # run_archive_pass(), resurrect_if_archived(), prune phase
      test_git.rs                 # init_test_repo() helper for unit tests
      domain/                     # pure domain types (no I/O)
        mod.rs  task.rs  state.rs  tag.rs  filter.rs  date_parse.rs
      storage/                    # local TOML + SQLite + git backend + archive tiers
        mod.rs                    # open(), task_path(), state_path_for_repo(), load_scoring()
        machine_state.rs          # MachineState (combined state.toml) + load_/update_machine_state
        archive.rs                # segments, ArchiveConfig, pruned.jsonl manifest (see §6.5)
        filenames.rs  lock.rs (FileLock)  toml_store.rs  cached_store.rs  git_backend.rs
      plugin/                     # export hook + periodic sync (machine-local registry)
        mod.rs  registry.rs  notify.rs  run.rs (run_due_syncs, resolve_sync_interval)
    cli/                          # feature = "cli" (default); the `next` binary + clap
      main.rs                     # `next` binary entry point
      app_context.rs              # AppContext: Config + TaskRepository (field `repo`); config.toml loading
      mod.rs  render.rs
      commands/
        add.rs   cancel.rs  config.rs  context.rs  data.rs   delete.rs  done.rs  edit.rs
        archive.rs  forecast.rs  init.rs  list.rs  maintenance.rs  mod.rs  move_cmd.rs  next_cmd.rs  open.rs
        resource.rs  show.rs  start.rs  stop.rs  sync.rs  tree.rs  tutorial.rs  user.rs
        plugin/mod.rs   tag/{mod,meta,data}.rs
    mcp/                          # feature = "mcp"; `next-mcp` binary
      main.rs  mod.rs  config.rs (McpConfig)  git_init.rs  protocol.rs  auth.rs
      sync_manager.rs  server.rs  tools/{mod,tasks,state,tags,data,view}.rs
    forgejo/                      # feature = "forgejo"; `next-forgejo` binary
      main.rs  mod.rs  config.rs  issues.rs  tasks.rs  reconcile.rs  hook.rs
    tui/                          # feature = "tui"; `next-tui` binary (ratatui front-end)
      main.rs                     # `next-tui` entry: arg parse (--repo/--config/--version) + run()
      mod.rs                      # VERSION (= CARGO_PKG_VERSION), run(), synchronous poll event_loop()
      app.rs                      # App state + Mode/View/Action; handle_key -> Action -> update()
      ui.rs                       # draw(): renders the active view, detail pane, modals/popups
      config.rs                   # ConfigSource + tui.toml -> config.toml -> defaults loader; bootstrap()
      edit.rs                     # EditForm: the edit-modal field model + validation
      tree.rs  forecast.rs        # TreeView / ForecastView state (build on cached tasks)
      state_panel.rs              # StatePanel: contexts/resources/users panel model
      sync.rs                     # background sync worker (worker thread + mpsc channel)
  benches/
    large_repo.rs                 # archiving budget benchmark (harness = false; opt-in)
  tests/
    common/mod.rs                 # shared test helpers (TestEnv, setup(), hook-safe git())
    test_*.rs                     # one integration file per command (add, list, edit, done, …)
    test_archive.rs               # archive pass, prune, resurrection, partial-clone recovery
    test_archive_scale.rs         # 2100-task lifecycle + multi-cycle resurrection
    test_multi_instance.rs        # several clones of one remote converging without loss
    test_cache_sync.rs  test_locking.rs  test_migration.rs  test_sync.rs
    test_plugin.rs                # export hook + periodic plugin sync
    test_mcp.rs                   # in-process MCP HTTP integration tests (requires --features mcp)
    test_container.rs             # container integration tests (requires CONTAINER_TESTS=1)
```

### 2.1 Cargo features

| Feature | Default | Adds | Optional deps pulled in |
|---------|---------|------|--------------------------|
| `cli` | ✓ | `next` binary, `src/cli/**` | `clap`, `tracing-subscriber` |
| `mcp` | | `next-mcp` binary, `src/mcp/**` | `tokio`, `axum`, `tracing-subscriber` |
| `forgejo` | | `next-forgejo` binary, `src/forgejo/**` | `forgejo-api`, `url`, `tokio`, `clap`, `tracing-subscriber` |
| `tui` | | `next-tui` binary, `src/tui/**` | `ratatui`, `tui-input`, `tui-textarea-2`, `tui-tree-widget`, `tracing-subscriber` |

The `tui` feature builds the `next-tui` terminal UI. Like the other feature modules it
depends only on `core`: it opens a [`TaskRepository`] via `core::bootstrap` and runs every
mutation through the same `core::service` / transaction code paths (and git commits) as the
CLI and MCP server, never shelling out to `next`. The event loop is a **synchronous poll
loop** (`mod.rs::event_loop`, ~200 ms tick) — a worker thread is used **only** for
background sync (`src/tui/sync.rs`, results delivered over an `mpsc` channel and drained each
tick). It reuses the shared refactors lifted into core for both CLI and TUI:
`core::recurrence::parse_recurrence` (recurrence-flag parsing), `core::forecast` (forecast
projection), `core::scoring::nonzero_factors` (score-breakdown rows), and `core::bootstrap`
(repo/config resolution + opening).

With **no features** (`--no-default-features`) the crate is just the core library —
`domain`, `storage`, `store`, `plugin`, `config`, `resolve`, `error`, `scoring`, `service`,
`task_repository` — with no `clap`/CLI dependencies, so other crates can link it.
`AppContext` (the CLI's `Config` + `TaskRepository` wrapper) is **not** part of core; it
lives at `src/cli/app_context.rs`, compiled only with the `cli` feature. The four feature
modules depend only on this core (the cross-cutting helpers they share — filter-token
parsing, data-value parsing, repo sync — live in `core`, never in `cli`). The presubmit
(`prek.toml`) runs clippy+test with `--all-features` and a `--no-default-features` clippy to
keep the core build clean.

Every **non-optional** dependency is required by `core`, so it is present even in the
featureless build; there are no CLI-exclusive always-on dependencies. The CLI-only crate
`clap` is `optional` and pulled in by the `cli` feature (and also by `forgejo`, whose binary
uses clap too); `tracing-subscriber` is shared by all four binaries. The `dep:` syntax in
`[features]` keeps these optional crates out of the dependency graph unless their feature is
enabled.

---

## 3. Module responsibilities

### `next` — core library

**Domain types** (`next::domain`):

| Module | Contents |
|--------|----------|
| `task` | `Task`, `Status` (`Open`/`Started`/`Done`/`Cancelled`), `Priority`, `Recurrence`, `Snap` |
| `state` | `GlobalState` (active contexts, excluded contexts, active users, resource availability map) |
| `tag` | `TagKind` (Context / Resource / Freeform); `validate_tag` (allowlist: segments start with letter, contain `a-zA-Z0-9-_`, `/` separator allowed, `..` explicitly rejected); `validate_context_tag` (enforces `@` prefix); `validate_resource_tag` (enforces `#` prefix) |
| `filter` | `FilterSet`, `fn apply(tasks, filter, state) -> Vec<Task>` |
| `date_parse` | `fn parse_date(expr, today) -> Result<NaiveDate>` |

**Core (non-domain) modules** at `next::core` (`src/core/*.rs`, not under `domain/`):

| Module | Contents |
|--------|----------|
| `scoring` | `ScoredTask`, `ScoringConfig`, `fn score(task, parent, today, weights, tag_metas)`, `fn score_and_sort(tasks, all_tasks, today, weights, tag_metas)` |
| `service` | `CreateTaskParams`, `EditTaskParams`, `create_task()`, `complete_task()`, `apply_edits()`, `validate_slug()`, `validate_url()`, `begin_mutation()`/`end_mutation()` — shared business logic used by the CLI, MCP, and Forgejo handlers |
| `recurrence` | `fn next_occurrence(rrule, anchor, after)`, `fn apply_snap(date, snap)`, `fn spawn_next(task, today)` |
| `task_repository` | `TaskRepository` — store + vcs + repo_root + scoring + transactions + plugin events |

Key `Task` fields: `id`, `title`, `status`, `priority`, `due`, `start`, `long_term`,
`slug`, `parent_id`, `assignee`, `tags`, `blocked_by`, `score_adjustment`, `description`,
`url`, `notes`, `data` (arbitrary JSON map; `data["time_log"]` accumulates start/stop events),
`recurrence`, `completed_at` (date set when marked done), `created_at`, `updated_at`.

Key `Task` methods: `is_open()` (Open only), `is_active()` (Open or Started), `mark_started()`,
`mark_stopped()`, `mark_done(completion_date)`, `mark_cancelled()`.

**Storage traits** (`next::store`):

```rust
pub trait Store: Send + Sync {
    // Tier-transparent reads (active + warm + cold)
    fn get_task(&self, id: Uuid) -> Result<Task>;
    fn get_task_by_slug(&self, slug: &str) -> Result<Option<Task>>;
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>>;
    fn get_tasks(&self, ids: &[Uuid]) -> Result<Vec<Task>>;       // batch; skips missing
    fn task_dates(&self) -> Result<HashMap<Uuid, TaskDates>>;     // git-derived created/updated
    fn task_location(&self, id: Uuid) -> Result<TaskLocation>;    // Active | Archived(segment)

    // Active-tier reads and paginated queries
    fn list_tasks(&self) -> Result<Vec<Task>>;                    // active tiers only
    fn query_tasks(&self, q: &TaskQuery) -> Result<Page<Task>>;   // SQL pushdown; see §7

    // Writes (guarded: saving/deleting an archived task is an error)
    fn save_task(&mut self, task: &Task) -> Result<()>;
    fn delete_task(&mut self, id: Uuid) -> Result<()>;

    // Archive-tier hooks used by core::archiver
    fn note_archived_segment(&mut self, rel_path: &str, entries: &[ArchivedTask]) -> Result<()>;
    fn note_cold_segment(&mut self, rel_path: &str) -> Result<()>;
    fn resurrect_task(&mut self, task: &Task) -> Result<()>;

    fn get_state(&self) -> Result<GlobalState>;
    fn save_state(&mut self, state: &GlobalState) -> Result<()>;
    // Core tag metadata (stored in tags/<tag>.toml; committed to git)
    fn get_tag_meta(&self, tag: &str) -> Result<Option<TagMeta>>;
    fn set_tag_meta(&mut self, tag: &str, meta: TagMeta) -> Result<()>;
    fn delete_tag_meta(&mut self, tag: &str) -> Result<()>;
    fn list_tag_metas(&self) -> Result<HashMap<String, TagMeta>>;

    // Cache/HEAD reconciliation (after_pull is incremental; see §6)
    fn after_pull(&mut self, new_head: &str) -> Result<()>;
    fn note_head(&mut self, new_head: &str) -> Result<()>;

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
    fn diff(&self) -> Result<String>;       // status --short + diff HEAD
    fn force_pull(&self) -> Result<String>; // fetch + reset --hard FETCH_HEAD
}
```

`TaskQuery` carries the cheap filter gates (statuses, archive tier, parent,
hierarchical tags) plus 1-indexed `page` / `page_size` (default 50);
`Page<T>` returns `items` with `page`/`page_size`/`total` so callers can tell
a truncated result from a complete one. The trait ships an in-memory default
implementation as the reference semantics; the SQLite cache overrides it with
indexed SQL, and an equivalence test pins the two together.

**Configuration** (`next::core::config`) — machine-local `config.toml`, CLI-only:

```rust
pub struct Config {
    pub sync: SyncConfig,              // see below
    pub forecast_horizon_days: u32,    // default 90
    pub next_count: usize,             // default 10
    pub list_limit: Option<usize>,     // default `next list` page size; None = 50
    pub repository: Option<PathBuf>,   // default repo root (overridden by --repo)
    pub autosync: bool,                // sync after each mutation (overridden by --autosync)
}

pub struct SyncConfig {
    pub git_subprocess: bool,          // use shell git for push/pull (default false)
    pub pull_before_query: bool,       // staleness pull before commands (default true)
    pub staleness_secs: u64,           // freshness window (default 3600)
    pub pull_timeout_secs: u64,        // stored only — not yet enforced (default 10)
    pub plugin_sync_default_secs: u64, // system-default plugin sync interval (default 86400)
    pub offline: bool,                 // behave as --offline on every invocation (default false)
}
```

`next config get/set` (`src/cli/commands/config.rs`) reads and writes this file from
the command line; it runs before `AppContext` because it needs no repository.

Scoring weights are **not** in `config.toml`. They live in the repository at
`config/scoring.toml` (committed, synced) and are loaded by
`storage::load_scoring()` into `TaskRepository::scoring` for every consumer
(cli/mcp/forgejo), giving a single consistent scoring view. `next init` seeds the
file with the defaults; absent or partial files fall back to `ScoringConfig::default()`.

**Error type** (`next::error`) — `pub type Result<T> = std::result::Result<T, TaskError>`:

```rust
pub enum TaskError {
    InvalidDate(String, String),       // expression + reason
    TaskNotFound(String),
    AmbiguousId(String, usize),
    SlugConflict(String),
    GitConflict(Vec<PathBuf>),
    Io(std::io::Error),
    Other(String),
}
```

### `next::storage` — local TOML + SQLite cache + git backend

**`CachedStore`** is the `Store` implementation returned by `open()`. It wraps
`TomlStore` and maintains an SQLite database at `<repo>/.next.db`:

- **Reads** (`list_tasks`, `query_tasks`, `get_task`, `get_task_by_slug`, prefix
  lookups, `task_dates`) query SQLite directly — no per-task TOML file reads.
  `query_tasks` compiles `TaskQuery` gates to indexed SQL (`EXISTS`/`NOT EXISTS`
  on `task_tags` for hierarchical tag matching) with `COUNT(*)` totals and
  `LIMIT`/`OFFSET` paging.
- **Writes** (`save_task`, `delete_task`, `save_state`) write to TOML first
  (authoritative), then update the SQLite cache in-place. The previous file
  location and slug uniqueness are resolved from the cache's `path` column —
  no directory scan — so a write costs O(1) in the task count.
- **Cache reconciliation**: when the stored git HEAD hash differs from the
  current one (a pull, or another process's commit), the two heads are
  tree-diffed with git2 and only the changed files are re-parsed — cost
  proportional to the change, not the corpus. Deletions resolve rows through
  the `path` column so renames net out and preserve `created_at`. The full
  rebuild survives only as the fallback when the stored head is missing or
  unresolvable (fresh clone); it parses task files and archive segments and
  backfills the date columns from one `git log` walk. A `schema_version` meta
  key drops and rebuilds the tables on layout changes.

SQLite schema (v3):

```sql
CREATE TABLE meta  (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE tasks (
    id           TEXT PRIMARY KEY,
    slug         TEXT,
    status       TEXT NOT NULL,          -- open|started|done|cancelled
    priority     TEXT NOT NULL,
    due          TEXT,                   -- ISO dates sort lexicographically
    start        TEXT,
    completed_at TEXT,
    parent_id    TEXT,
    assignee     TEXT,
    archived     INTEGER NOT NULL,       -- 0 active · 1 warm segment · 2 cold (pruned)
    path         TEXT NOT NULL,          -- repo-relative file or segment path
    created_at   TEXT,                   -- git-derived; frozen once archived
    updated_at   TEXT,
    data         TEXT NOT NULL           -- full Task serialised as JSON
);
CREATE TABLE task_tags (task_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (task_id, tag));
CREATE INDEX idx_tasks_slug    ON tasks(slug);
CREATE INDEX idx_tasks_status  ON tasks(archived, status);
CREATE INDEX idx_tasks_parent  ON tasks(parent_id);
CREATE INDEX idx_tasks_path    ON tasks(path);
CREATE INDEX idx_task_tags_tag ON task_tags(tag);
```

The `created_at`/`updated_at` columns hold git-derived timestamps (scoring's
age factor reads them via `Store::task_dates`): local saves stamp them, the
incremental reconcile stamps changed files with the new head's commit time
while preserving `created_at`, and archived entries carry frozen dates inside
their segments. No listing ever walks git history.

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

All machine-local state lives in a single `state.toml` at
`$XDG_STATE_HOME/task-manager/<fnv1a-hash-of-canonical-repo-path>/state.toml`, computed by
`next::storage::state_path_for_repo(root)` and never committed to git.  It holds three
sections (see `storage/machine_state.rs`, `MachineState`):

* the global runtime state — active contexts, excluded contexts, resource availability,
  active users — flattened at the top level (unchanged on-disk format);
* the plugin registry as a `[[plugin]]` array;
* the sync state under `[sync]` (`last_pull`, `last_archive` for the daily
  auto-archive throttle, plus per-plugin `last_sync`).

`load_machine_state` / `update_machine_state` are the read / locked-read-modify-write
primitives all three subsystems (`TomlStore` global state, `plugin::registry`,
`sync_state`) share, so none can clobber another's section. A single advisory lock file
co-located with `state.toml` (`.state.toml.lock`) guards every machine-local write.

Two one-time migrations run on `TomlStore::open()`:
1. **State file migration**: if `<repo>/state.toml` exists and the XDG path does not,
   the file is moved to the XDG location.
2. **Tag description migration**: if `state.toml` contains a legacy `[tag_descriptions]`
   table, each entry is extracted to its own file under `tags/` and the table is removed.

Both migrations are idempotent (subsequent opens are no-ops).

**`GitBackend`** wraps `Mutex<git2::Repository>` to satisfy `Send + Sync`. Commit
messages follow the pattern `next: <verb> "<task title>"`.

### `next::cli` — command handlers (feature = `"cli"`)

See the §2 tree for the full file list. Key entry points:

- `src/cli/main.rs` — the `next` binary entry point. `Init`/`Tutorial`/`Config` are handled
  before `AppContext`; all other commands build an `AppContext` and dispatch.
- `src/cli/mod.rs` — top-level `Cli` + `Command` enum (clap derive), including the global
  `--repo`, `--autosync`, `--no-autosync`, `--offline`, and `--no-sync` flags.
- `src/cli/render.rs` — task list and detail rendering (text and `--json`).
- `src/cli/commands/` — one module per subcommand (`add`, `done`, `edit`, …), plus the
  `tag/` (`mod`/`meta`/`data`) and `plugin/` submodules.

ID resolution (`resolve_task_id`) and filter-token conversion (`FilterArgs`) are **not** in
`cli` — they live in `next::core::resolve` (`src/core/resolve.rs`) and
`next::core::filter_args` (`src/core/filter_args.rs`) so every consumer shares them. There is
no `src/log.rs` / logging module.

---

## 4. Application context

`AppContext` is the CLI-only wrapper around `TaskRepository`. Mutation command handlers
receive `&mut AppContext`; read-only commands take `&AppContext`:

```rust
pub struct AppContext {
    pub config: Config,         // the loaded config.toml (CLI-only)
    pub repo: TaskRepository,   // the opened repository (store + vcs + repo_root + scoring)
}
```

There is no `Deref` and no `store`/`vcs`/`repo_root` fields on `AppContext`. Handlers reach
the repository through `ctx.repo` — e.g. `ctx.repo.store`, `ctx.repo.store_mut()`,
`ctx.repo.transaction(…)`, `ctx.repo.state_transaction(…)`, `ctx.repo.record_task_event(…)` —
and CLI configuration via `ctx.config`.

`AppContext::new(config_path, repo)`:

1. Load `config.toml` (the given path, else the XDG default; falls back to `Config::default()`).
2. Resolve the repository root: the `--repo` override, else `config.repository`, else walk up
   from CWD for a `.git` directory; fail if none is found.
3. Call `next::core::storage::open(root)` to get `(CachedStore, GitBackend)`, apply
   `config.sync.git_subprocess` to the git backend, and assemble
   `TaskRepository::with_parts(store, vcs, root)` (which also loads `config/scoring.toml` into
   `repo.scoring`).

The MCP server and the Forgejo plugin build a `TaskRepository` directly (via `with_parts` /
`TaskRepository::open`) without `AppContext`, since they do not read `config.toml`.

Remote access is provided via MCP — connect with `claude mcp add --transport http https://next-mcp.victorsavu.eu`.

---

## 5. Command execution lifecycle

`next init`, `next tutorial`, and `next config` run before `AppContext` is constructed:

```
1. Parse CLI args (clap); no subcommand defaults to `list`
2. If command is Init → run init::run(args, cwd); exit
   If command is Tutorial → print embedded TUTORIAL.md; exit
   If command is Config → read/write config.toml; exit
3. AppContext::new(): locate repository root, open CachedStore + GitBackend into TaskRepository
4. Pull-before-query (all commands except `sync`): core::sync::pull_if_stale() pulls when
   the local copy is older than `staleness_secs`; best-effort, skipped by --offline/--no-sync
5. Execute command logic (reads from ctx.repo.store; writes via ctx.repo.transaction)
6. Task mutations (add/edit/start/stop/done/cancel/delete/move/tag/data): vcs.commit(changed_paths, message)
   State mutations (context/resource/user): write to XDG state file only; no commit
7. Render output (text or JSON to stdout)
8. If autosync enabled and command succeeded and is a mutation (`add`/`start`/`stop`/`done`/`cancel`/`edit`/`delete`/`move`/`tag`/`data`/`archive`): run sync (pull + push)
9. Notify subscribed plugins of recorded task events (after the repo lock is released)
10. On error: print the message to stderr and propagate to main for a non-zero exit code
```

Read-only commands (list, show, forecast, tree) skip steps 6 and 8 and take `&AppContext` rather than `&mut AppContext`.

---

## 6. Storage layer

`next::core::storage::open(root)` returns `(CachedStore, GitBackend)`. `CachedStore` satisfies
the `Store` trait; callers box it as `Box<dyn Store>` inside `TaskRepository`.

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

1. **Re-entrant advisory lock** (`storage::FileLock`, `src/core/storage/lock.rs`). A generic
   advisory `flock(2)` over a given lock file, exclusive across processes and threads but
   **re-entrant within a single thread** so a transaction can hold it while the nested
   `save_task` / `commit` / `save_state` calls re-acquire it. A process-global registry
   maps each lock-file path to one in-process gate (owner thread + recursion depth)
   layered over the OS lock — plain `flock` is per open-file-description and would
   otherwise self-deadlock on the second acquire. Two *separate* instances are used: the
   **repo lock** (`<repo>/.next.lock`, shared by `TomlStore` task/tag writes and
   `GitBackend` commit/pull/push) and the **state lock** (`.state.toml.lock`), which now
   guards *all* machine-local writes — global state, plugin registry, and sync state —
   since they share one `state.toml` (see below).

2. **Repository mutation transactions** (`TaskRepository::transaction`, built on
   `service::begin_mutation` / `end_mutation`). Every task or tag mutation holds the repo
   lock across the *entire* read → modify → write → commit sequence, so two processes
   cannot interleave and lose each other's updates. On entry the transaction reconciles
   the cache with the on-disk git HEAD (`Store::after_pull`) so the read reflects other
   processes' commits; on exit it records the new HEAD (`Store::note_head`) so the next
   transaction does not rebuild needlessly.

3. **State mutation transactions** (`TaskRepository::state_transaction`). All machine-local
   state lives outside the git repository in one `state.toml`, so it has its own lock —
   `.state.toml.lock` next to the state file. This single lock now serialises every
   machine-local write: the global state (active contexts, excluded contexts, active users,
   resource availability), the plugin registry, and the sync state. Each writer does a
   locked read-modify-write of the whole file via `load_machine_state` /
   `update_machine_state` (`storage/machine_state.rs`), so e.g. a `save_state` cannot drop a
   concurrently-added plugin and vice versa. Each `next context` / `resource` / `user` (and
   the matching MCP tool) holds this exclusive lock across its `get_state` → modify →
   `save_state`, closing the same lost-update window. No HEAD reconciliation, since state is
   never committed to git.

4. **SQLite WAL + busy_timeout** (`CachedStore::configure_connection`). `.next.db` is a
   single file shared by all processes; WAL lets readers and a writer proceed
   concurrently and the 5 s `busy_timeout` waits out transient locks instead of failing
   with `SQLITE_BUSY` (e.g. `next list` running during an MCP mutation). The WAL sidecars
   (`.next.db-wal` / `.next.db-shm`) are git-ignored.

5. **Plugin notification** (`plugin::notify`, see §6.4). Subscribed plugins are spawned
   only at the post-mutation chokepoints (`main.rs` after autosync; MCP `tools::dispatch`
   after the tool runs), i.e. **after the repo lock is released** — a plugin typically
   calls back into `next` and would otherwise deadlock. The plugin registry now lives in
   `state.toml`, so registry reads/writes (during notification and `next plugin` edits)
   take the shared state lock.

The two locks are independent files and never block one another. When a path takes both,
the ordering is always repo-lock-before-state-lock, never the reverse, so they cannot
deadlock. `tests/test_locking.rs` covers lost-update prevention for task, state, and
plugin-subscription edits, the cross-section non-clobbering of the shared `state.toml`,
slug-conflict races, and pull/commit coordination; re-entrant lock unit tests live in
`src/core/storage/lock.rs`.

### 6.4 Plugins (export hook)

External plugin binaries subscribe to individual tasks and are notified when those tasks
change. The registry (`plugin::registry`) is machine-local — the `[[plugin]]` section of
the combined `state.toml` in the per-repo state dir, never committed — each entry being
`{ name, command: argv, tasks: [uuid] }`, managed by the `next plugin` subcommands.
Registry reads/writes go through the shared `storage::machine_state` helpers under the
single state lock, so they cannot clobber the global or sync sections of the file.

Each task-mutating handler records a `(verb, task_id)` `TaskEvent` on `TaskRepository` after its
transaction returns. The CLI (`main.rs`) and MCP (`tools::dispatch`) chokepoints drain the
buffer and call `plugin::notify`, which spawns every subscribed plugin's command
fire-and-forget (event JSON on stdin + `NEXT_PLUGIN_EVENT`/`NEXT_REPO`/`NEXT_PLUGIN_ORIGIN`
env). The origin plugin is skipped (loop guard via `NEXT_PLUGIN_ORIGIN`), the long-lived
server reaps children on a helper thread, and `delete` events prune the subscription.

**Periodic plugin sync** (`plugin::run`): a plugin may also carry a `sync_command`
(import direction, e.g. `next-forgejo sync`). After a clean `next sync`, `run_due_syncs`
runs each enabled, due plugin's sync command synchronously (interval resolved
USER override → PLUGIN default → `config.sync.plugin_sync_default_secs`); only a
successful exit records `last_sync` in the machine-local sync state, so failures retry
on the next sync. See REQUIREMENTS.md §10 for the full contract.

### 6.5 Archive tiers

Requirements are in REQUIREMENTS.md §2.3; the machinery lives in
`storage::archive` (formats), `core::archiver` (the pass), and `CachedStore`
(tier-aware rows).

- **Warm segments** (`archive/<YYYY>/<MM>-<NNN>.toml`) are TOML
  array-of-tables of `ArchivedTask` — the full task flattened, plus
  `created_at`/`updated_at` frozen from git at archive time. Writes sort by
  `(completed_at, id)` and are byte-deterministic; an emptied segment removes
  its file. `config/archive.toml` (committed) holds the policy.
- **The archive pass** (`archiver::run_archive_pass`) runs as one repository
  transaction: select eligible tasks (see the eligibility guards in
  REQUIREMENTS §2.3), delete their files/rows, pack them into each month's
  open segment (sealing at the cap, mirroring rows via
  `Store::note_archived_segment`), commit once. It runs automatically from
  `core::sync` after a clean pull — throttled to one automatic run per day
  via `[sync] last_archive`, gated by the `auto` flag, stamped *before*
  running so failures don't retry every sync — or on demand via
  `next maintenance archive`.
- **The prune phase** follows the warm commit when `prune_after_days` is set:
  segments past the threshold leave the checkout, each recorded in the
  append-only `archive/pruned.jsonl` (path, blob SHA at HEAD, task count;
  `merge=union`, last line per path wins). Cache rows flip to tier 2 in
  place. Segment numbers listed in the manifest but absent from the checkout
  are never reused.
- **Resurrection** (`archiver::resurrect_if_archived`, called by
  `service::apply_edits` inside the mutation's transaction): the entry leaves
  its segment (fetched from its blob when cold — `git cat-file` falls back to
  an on-demand promisor fetch in partial clones), the individual file is
  restored, the row flips tiers keeping its frozen `created_at`, and the
  touched segment joins the mutation's commit. `save_task`/`delete_task` on
  an archived task fail loudly instead of duplicating the task or (via the
  path hint) deleting a whole segment.
- **Budgets** are pinned by `cargo bench --bench large_repo` (no bench-only
  dependencies; `NEXT_BENCH_TASKS`, `NEXT_BENCH_STRICT=1`). Measured at 10⁶
  tasks: rebuild 53 s, 200-file reconcile 17 ms, edit 17 ms, filtered query
  9 ms, archived page 0.6 s.

---

## 7. Filtering pipeline

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

## 8. ID resolution

`src/core/resolve.rs`:

```rust
pub fn resolve_task_id(store: &dyn Store, id_str: &str) -> anyhow::Result<Uuid>
```

Resolution order:
1. Full UUID — `Uuid::parse_str`
2. Slug — `store.get_task_by_slug`
3. UUID prefix (4+ chars) — `store.find_tasks_by_prefix`; error if > 1 match

---

## 9. Scoring

Scores are computed at query time (not stored) by `core::scoring::score_and_sort`.

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

## 10. Natural-language date parsing

`domain::date_parse::parse_date(expr: &str, today: NaiveDate) -> Result<NaiveDate>`

Thin wrapper over `interim::parse_date_string`:

1. Try ISO 8601 (`YYYY-MM-DD`) first to avoid ambiguity
2. Convert `today: NaiveDate` to `DateTime<Local>` (midnight) — required by interim's API
3. Call `interim::parse_date_string(expr, dt, Dialect::Uk)`
4. Extract `.date_naive()` from result

Accepted by `--due` and `--start` in `next add` and `next edit`.

---

## 11. Configuration

Two layers: machine-local CLI settings, and repo-stored scoring weights.

### 11.1 Machine-local — `$XDG_CONFIG_HOME/task-manager/config.toml`

CLI-only (the MCP server and plugins do not read it); loaded once at startup, falls
back to defaults when absent. Editable in place with `next config get/set`.

```toml
repository            = "/home/alice/tasks"  # use next from any directory
autosync              = false                # sync after each mutation (--autosync to override)
list_limit            = 20                   # default `next list` page size; absent = 50
forecast_horizon_days = 90
next_count            = 10                   # tasks shown by `next next`

[sync]
git_subprocess           = false   # shell git instead of libgit2 for push/pull
pull_before_query        = true    # staleness pull before commands (--offline to skip)
staleness_secs           = 3600    # freshness window for pull-before-query
pull_timeout_secs        = 10      # stored only — not yet enforced
offline                  = false   # behave as if --offline on every invocation
plugin_sync_default_secs = 86400   # system-default periodic plugin sync interval
```

### 11.2 Repo-stored scoring — `<repo>/config/scoring.toml`

Committed to the repository and synced, so the CLI, the MCP server, and plugins all
score tasks identically. Loaded by `storage::load_scoring()` into
`TaskRepository::scoring`. `next init` seeds it with the defaults below; omitted
fields fall back to `ScoringConfig::default()`.

```toml
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

## 12. Error handling

```rust
pub enum TaskError {
    #[error("invalid date expression '{0}': {1}")]
    InvalidDate(String, String),       // exit 1

    #[error("task not found: {0}")]
    TaskNotFound(String),              // exit 1

    #[error("ambiguous task ID prefix '{0}': matches {1} tasks")]
    AmbiguousId(String, usize),        // exit 1

    #[error("slug '{0}' is already taken by another task")]
    SlugConflict(String),              // exit 1

    #[error("git conflict in files: {0:?}")]
    GitConflict(Vec<PathBuf>),         // exit 2

    #[error(transparent)]
    Io(#[from] std::io::Error),        // exit 2

    #[error("{0}")]
    Other(String),                     // exit 2
}
```

All error messages are printed to stderr; `main` propagates the `anyhow::Error` to produce a
non-zero exit code.

---

## 13. Testing strategy

| Layer | Location | Approach |
|-------|----------|----------|
| `core::scoring` | `src/core/scoring.rs` | Unit tests with fixed dates; each factor tested independently |
| `domain::filter` | `src/core/domain/filter.rs` | Unit tests: build `FilterSet` + `Vec<Task>`, assert filtered output |
| `domain::date_parse` | `src/core/domain/date_parse.rs` | Unit tests: fixed "today", assert parsed date for common expressions |
| `TomlStore` | `src/core/storage/toml_store.rs` | Round-trip tests: write task to `tempdir`, read back, assert equal fields; migration unit tests: write legacy `state.toml`, call `TomlStore::open()`, assert per-tag files created and `state.toml` cleaned |
| `GitBackend` | `src/core/storage/git_backend.rs` | Integration tests against `tempdir` git repo; assert commits and HEAD |
| `CachedStore` | `src/core/storage/cached_store.rs` | Unit tests: save/retrieve/delete/rebuild within a `tempdir` git repo |
| Cache sync | `tests/test_cache_sync.rs` | Integration tests: write-through consistency (SQLite ↔ TOML), git pull propagation (HEAD change triggers rebuild), cache-reuse (same HEAD = no rebuild) |
| Archiving | `tests/test_archive.rs` | Integration tests: archive pass, segment sealing, frozen dates, resurrection (warm + cold), blind-write guards, sync auto-archive throttle, cold pruning, manifest numbering, partial-clone cold recovery |
| Archiving at scale | `tests/test_archive_scale.rs` | 2100-task corpus through the full lifecycle (tier accounting, gap-free pagination, cross-machine reconcile); three resurrection / re-archive / re-prune cycles with no duplicates |
| Budgets | `benches/large_repo.rs` | Opt-in benchmark (`cargo bench --bench large_repo`); pins the 1M-task budgets from REQUIREMENTS §2.3 |
| Migration | `tests/test_migration.rs` | Integration tests: write legacy `state.toml` with `[tag_descriptions]`, call `next::storage::open()`, assert per-tag files, state cleanup, idempotency, and persistence across reopens |
| File locking | `tests/test_locking.rs` | Concurrency tests: multiple threads open independent `TomlStore`/`GitBackend` instances (simulating separate processes) and assert no data loss or corruption, including transactional lost-update prevention (N processes each add a distinct tag to one task; all must survive). Re-entrant lock unit tests live in `src/core/storage/lock.rs` |
| CLI commands | `tests/test_*.rs` | Integration tests: construct `AppContext` directly in a `tempdir` git repo; call `run()` functions; assert store state |
| MCP unit tests | `src/mcp/tools/*.rs` | Unit tests per tool module using a real `TaskRepository` in a `tempdir` git repo (requires `--features mcp`) |
| Plugins | `tests/test_plugin.rs`, `src/core/plugin/run.rs` | End-to-end export-hook notification; unit tests for periodic sync (interval precedence, due/skip, failure-retry) |
| Multi-instance | `tests/test_multi_instance.rs` | Several clones of one shared remote mutating in parallel (adds, edits, archive passes, resurrections); all instances must converge with no loss |
| MCP integration tests | `tests/test_mcp.rs` | Start a real HTTP server on `127.0.0.1:0` in `#[tokio::test]`; test all 15 tools, auth, webhook, and autosync (requires `--features mcp`) |
| Container tests | `tests/test_container.rs` | Start the real container image via `testcontainers` (Podman); opt-in with `CONTAINER_TESTS=1 DOCKER_HOST=unix:///…/podman.sock`; covers git clone, auth, sync push, deferred timer, webhook, and idempotent restart |

---

## 14. Dependencies

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
