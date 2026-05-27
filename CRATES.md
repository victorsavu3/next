# Crate structure

## Workspace layout

```
next/                             # workspace root
  Cargo.toml                      # [workspace] manifest
  crates/
    next/                         # library: domain logic + storage traits + config
    next-storage/                 # library: TOML + git2 backend + SQLite cache
    next-remote-storage/          # library: stub HTTP backend
    next-cli/                     # binary: clap CLI wrapper
```

---

## Dependency graph

```
next-cli
  ├── next                  (domain types + traits + config)
  ├── next-storage          (local Store + VcsBackend implementations)
  └── next-remote-storage   (stub remote Store + VcsBackend)

next-storage
  └── next

next-remote-storage
  └── next

next
  └── (no internal workspace deps)
```

No crate has a circular dependency.

---

## Crate responsibilities

### `next` — core library

Everything a caller needs to use the task manager programmatically.

**Domain types** (`next::domain`):

| Module | Contents |
|--------|----------|
| `task` | `Task`, `Status`, `Priority`, `Recurrence` |
| `state` | `GlobalState` (active contexts, active users, resource map) |
| `tag` | `TagKind` (Context / Resource / Freeform); tag parsing helpers |
| `filter` | `FilterSet`, `fn apply(tasks, filter, state, today) -> Vec<Task>` |
| `scoring` | `ScoredTask`, `ScoringWeights`, `fn score_and_sort(tasks, all_tasks, today, weights)` |
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
    pub repository: Option<PathBuf>,  // default repo path; overridden by --repo flag
}
pub enum BackendKind { Local, Remote }
pub struct BackendConfig { pub kind: BackendKind, pub remote: Option<RemoteBackendConfig> }
pub struct RemoteBackendConfig { pub url: String, pub token: Option<String> }
```

**Application context** passed to every command handler:

```rust
pub struct AppContext {
    pub config: Config,
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    pub repo_root: PathBuf,
    pub log: Logger,
}
```

---

### `next-storage` — local TOML + SQLite cache + git backend

Implements `Store` and `VcsBackend` against real files and git.

```
crates/next-storage/src/
  lib.rs            # pub fn open(root: PathBuf) -> Result<(CachedStore, GitBackend)>
                    # pub fn task_path(root: &Path, task: &Task) -> PathBuf
                    # pub fn tag_description_path(root: &Path, tag: &str) -> PathBuf
  cached_store.rs   # CachedStore: wraps TomlStore with an SQLite read cache
  toml_store.rs     # TomlStore: source-of-truth TOML file I/O
  git_backend.rs    # GitBackend: implements VcsBackend via git2
```

**`CachedStore`** is the `Store` implementation returned by `open()`. It wraps
`TomlStore` and maintains an SQLite database at `<repo>/.next.db`:

- **Reads** query SQLite directly (no TOML file reads per query)
- **Writes** write to TOML first (authoritative), then update SQLite in-place
- **Cache invalidation**: on `open()`, HEAD hash mismatch triggers a full rebuild from TOML

**`TomlStore`** holds the repo root path. It reads and writes:
- One `.toml` file per task under `tasks/`
- `state.toml` at the root (active contexts, resource availability)
- One `.toml` file per tag description under `tags/` — the tag string maps directly to a
  path (`@work` → `tags/@work.toml`, `@home/kitchen` → `tags/@home/kitchen.toml`)

On first open of a repository that has a legacy `[tag_descriptions]` table in
`state.toml`, `TomlStore::open()` runs a one-time migration: each entry is extracted to
its own file under `tags/` and the table is removed from `state.toml`.

**`GitBackend`** wraps `Mutex<git2::Repository>` for `Send + Sync`. Commit messages
follow the pattern `next: <verb> "<task title>"`. `GitBackend` and `TomlStore` share a
single advisory lock file (`.next.lock`) so that git operations and task writes are
always serialised.

**`task_path`** and **`tag_description_path`** are exported so `next-cli` commands can
pass the correct paths to `vcs.commit()` after writing a task or tag file.

**External crates used only in `next-storage`**:

| Crate | Purpose |
|-------|---------|
| `git2` | git commit / pull / push |
| `toml` | TOML file serialisation |
| `rusqlite` | SQLite read cache |
| `fs4` | cross-platform advisory file locking (`flock(2)`) |

---

### `next-remote-storage` — stub HTTP backend

```
crates/next-remote-storage/src/
  lib.rs            # re-exports RemoteStore, RemoteVcs
  remote_store.rs   # RemoteStore: all Store methods return AppError::Other("not yet implemented")
  remote_vcs.rs     # RemoteVcs: commit/push no-ops; pull → PullResult::Clean; head_hash → "remote"
```

`RemoteStore { url, token }` — construction never fails; errors are returned lazily when
any method is called, so the binary starts up cleanly even before a server is reachable.

---

### `next-cli` — binary

A thin wrapper: parse CLI args with clap, construct `AppContext`, call the appropriate
command handler, render output.

```
crates/next-cli/src/
  main.rs           # entry point; AppContext::new(); dispatch; log errors
  log.rs            # Logger: append-only next.log; rotation at 1 MB to next.log.1
  resolve.rs        # fn resolve_task_id(store, id_str) -> Result<Uuid>
  cli/
    mod.rs          # top-level Cli struct + Command enum (clap derive)
    filter.rs       # FilterArgs -> FilterSet; parses +tag, -tag, user:name, context:@x tokens
    render.rs       # task list and detail rendering (text columns or --json)
    commands/
      add.rs        cancel.rs   context.rs  data.rs
      delete.rs     done.rs     edit.rs     export.rs
      forecast.rs   import.rs   init.rs     list.rs
      mod.rs        move_cmd.rs next_cmd.rs open.rs
      resource.rs   show.rs     sync.rs     tag.rs
      tree.rs       user.rs
```

Each `commands/*.rs` file contains:
1. A clap `Args` struct (pure parsing, no logic)
2. A `fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()>` that converts args
   to typed domain values, calls store/vcs methods, and renders output

**External crates used only in `next-cli`**:

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing |
| `serde_json` | `--json` output serialisation |
| `chrono` | Local date for scoring/filtering |
| `dirs` | XDG base directory resolution |
| `anyhow` | Error propagation |

---

## External crate allocation

| Crate | Used in |
|-------|---------|
| `clap` | `next-cli` only |
| `git2` | `next-storage` only |
| `toml` | `next-storage` only |
| `rusqlite` | `next-storage` only |
| `serde`, `serde_json` | `next` (domain types), `next-cli` (JSON output) |
| `chrono` | `next` (domain types), `next-cli` (date computations) |
| `interim` | `next` (date_parse module) |
| `uuid` | `next` (Task::id), `next-cli` (resolve), `next-remote-storage` |
| `dirs` | `next-cli` only (XDG path resolution) |
| `anyhow`, `thiserror` | `next`, `next-cli`, `next-storage`, `next-remote-storage` |
