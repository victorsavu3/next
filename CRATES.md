# Crate structure proposal

## Workspace layout

```
next/                          # workspace root
  Cargo.toml                   # [workspace] manifest
  crates/
    next/                      # library: domain logic + storage traits
    next-storage/              # library: concrete storage implementations
    next-test-utils/           # library: in-memory Store + VcsBackend stubs (dev only)
    next-cli/                  # binary: thin clap wrapper
```

---

## Dependency graph

```
next-cli
  ├── next               (library API + AppContext construction)
  ├── next-storage       (constructs concrete Store + VcsBackend)
  └── next-test-utils    (dev-dependency only)

next-storage
  ├── next               (domain types + storage traits)
  └── next-test-utils    (dev-dependency only)

next-test-utils
  └── next               (implements Store + VcsBackend traits)

next
  └── (no internal workspace deps)
```

`next-test-utils` is always a `[dev-dependencies]` entry — it is never a runtime
dependency and never compiled into production binaries.

`next` defines the traits; `next-storage` implements them; `next-cli` wires them together.
No crate has a circular dependency.

---

## Crate responsibilities

### `next` — core library

Everything a caller needs to use the task manager programmatically.

**Domain types** (`next::domain`):

| Module | Contents |
|--------|----------|
| `task` | `Task`, `Status`, `Stage`, `Priority`, `Recurrence` |
| `project` | `Project` |
| `state` | `GlobalState` (active contexts, resource map) |
| `tag` | `Tag`, `TagKind` (Context / Resource / Freeform) |
| `filter` | `FilterSet`, `fn apply(tasks, filter, state) -> Vec<Task>` |
| `scoring` | `ScoringWeights`, `fn score(task, project, weights) -> f64` |
| `recurrence` | `fn next_due(rule, after) -> Result<NaiveDate>`, `fn to_rrule(rule_str)` |
| `date_parse` | `fn parse_date(expr, today) -> Result<NaiveDate>` |

**Storage traits** (`next::store`):

```rust
/// All persistent data operations. Implemented by next-storage; mocked in tests.
pub trait Store: Send + Sync {
    fn get_task(&self, id: Uuid) -> Result<Task>;
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>>;
    fn list_tasks(&self) -> Result<Vec<Task>>;
    fn save_task(&self, task: &Task) -> Result<()>;
    fn delete_task(&self, id: Uuid) -> Result<()>;

    fn get_project(&self, path: &str) -> Result<Option<Project>>;
    fn list_projects(&self) -> Result<Vec<Project>>;
    fn save_project(&self, project: &Project) -> Result<()>;

    fn get_state(&self) -> Result<GlobalState>;
    fn save_state(&self, state: &GlobalState) -> Result<()>;
}

/// Version-control operations. Separate from Store so tests can mock independently.
pub trait VcsBackend: Send + Sync {
    fn commit(&self, paths: &[PathBuf], message: &str) -> Result<()>;
    fn pull(&self) -> Result<PullResult>;   // PullResult: Clean | Conflicts(Vec<PathBuf>)
    fn push(&self) -> Result<()>;
    fn head_hash(&self) -> Result<String>;
}
```

**Application context** passed to every command handler:

```rust
pub struct AppContext {
    pub store: Box<dyn Store>,
    pub vcs: Box<dyn VcsBackend>,
    pub config: Config,
}
```

**Command handlers** (`next::commands`):

One module per command group; each function takes `&mut AppContext` and typed arguments
and returns `Result<CommandOutput>`.

| Module | Functions |
|--------|-----------|
| `commands::task` | `add`, `show`, `edit`, `done`, `cancel`, `delete`, `move_task` |
| `commands::list` | `list`, `next_tasks` |
| `commands::project` | `project_add`, `project_list`, `project_show` |
| `commands::context` | `context_show`, `context_set`, `context_clear` |
| `commands::resource` | `resource_list`, `resource_set` |
| `commands::recurrence` | `forecast` |
| `commands::review` | `review` (takes an additional `&mut dyn ReviewUi` for testability) |
| `commands::sync` | `sync` |
| `commands::import` | `import_forgejo`, `import_ical` |
| `commands::export` | `export_ical` |

**Output types** (`next::output`):

```rust
pub enum CommandOutput {
    Tasks(Vec<ScoredTask>),
    Task(Task),
    Projects(Vec<Project>),
    Message(String),
    Json(serde_json::Value),
}
```

The caller (`next-cli`) is responsible for rendering `CommandOutput` to the terminal.

---

### `next-storage` — concrete storage

Implements `Store` and `VcsBackend` against real files, SQLite, and git.

```
crates/next-storage/src/
  lib.rs            # re-exports RepoStore, GitBackend
  repo_store.rs     # RepoStore: implements Store
  toml/
    mod.rs          # read/write Task + Project TOML files
    task_file.rs
    project_file.rs
    state_file.rs
  sqlite/
    mod.rs          # schema init, rebuild, stale-check
    schema.rs       # CREATE TABLE statements
    queries.rs      # typed query helpers
  git/
    mod.rs          # GitBackend: implements VcsBackend (via git2)
```

**`RepoStore`** holds the repo root path and a `rusqlite::Connection`. It:
1. Delegates reads to the SQLite cache (fast)
2. Delegates writes to the TOML files (source of truth) and then updates the cache row
3. Checks cache staleness on construction via `sqlite::is_stale(conn, &git_head)`

```rust
pub struct RepoStore {
    root: PathBuf,
    conn: rusqlite::Connection,
}

impl Store for RepoStore { ... }

pub struct GitBackend {
    repo: git2::Repository,
}

impl VcsBackend for GitBackend { ... }
```

**Construction** (called from `next-cli`):

```rust
pub fn open(root: PathBuf) -> Result<(RepoStore, GitBackend)> {
    let repo = git2::Repository::open(&root)?;
    let head = head_hash(&repo)?;
    let conn = sqlite::open_or_create()?;
    if sqlite::is_stale(&conn, &head)? {
        sqlite::rebuild(&conn, &root)?;
    }
    Ok((RepoStore { root, conn }, GitBackend { repo }))
}
```

**External crates used by `next-storage`** only:

| Crate | Purpose |
|-------|---------|
| `rusqlite` | SQLite cache |
| `git2` | git commit / pull / push |
| `toml` | TOML serialisation |

These crates do not appear in `next` or `next-cli`; they are an implementation detail.

---

### `next-cli` — binary

A thin wrapper: parse CLI args with `clap`, construct `AppContext`, call the appropriate
`next::commands::*` function, render output.

```
crates/next-cli/src/
  main.rs           # entry point; locate repo root, open store, dispatch
  cli/
    mod.rs          # top-level Cli struct (clap derive)
    filter.rs       # shared FilterArgs -> FilterSet conversion
    render.rs       # CommandOutput -> terminal (text or --json)
    commands/
      add.rs        # Args struct + fn run(args, ctx)
      list.rs
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
      import.rs
      export.rs
```

Each `commands/*.rs` file contains:
1. A `clap` `Args` struct (pure parsing, no logic)
2. A `fn run(args: Args, ctx: &mut AppContext) -> Result<CommandOutput>` that converts
   `Args` fields to typed domain values and calls `next::commands::*`

`main.rs` does nothing except:
1. Find the repository root (walk up from CWD for `state.toml` / `.git`)
2. Call `next_storage::open(root)` to get `(RepoStore, GitBackend)`
3. Build `AppContext { store: Box::new(repo_store), vcs: Box::new(git_backend), config }`
4. Dispatch to the matched command's `run` function
5. Render the returned `CommandOutput`

---

## Testing approach

### `next-test-utils` — shared in-memory stub

A fourth crate provides a complete in-memory implementation of `Store` and `VcsBackend`.
It is a dev-dependency of all three other crates; it never ships in production binaries.

```
crates/next-test-utils/src/
  lib.rs
  mem_store.rs      # MemStore: Store backed by HashMaps
  mem_vcs.rs        # MemVcs: VcsBackend that records calls in a Vec
```

**`MemStore`** is a real implementation — it stores state in memory and returns it
faithfully. Tests exercise actual domain logic paths rather than setting up expectations:

```rust
// next-test-utils/src/mem_store.rs
use std::collections::HashMap;
use next::{store::Store, domain::*};

#[derive(Default)]
pub struct MemStore {
    tasks:    HashMap<Uuid, Task>,
    projects: HashMap<String, Project>,
    state:    GlobalState,
}

impl Store for MemStore {
    fn get_task(&self, id: Uuid) -> Result<Task> {
        self.tasks.get(&id).cloned().ok_or_else(|| AppError::TaskNotFound(id.to_string()))
    }
    fn list_tasks(&self) -> Result<Vec<Task>> {
        Ok(self.tasks.values().cloned().collect())
    }
    fn save_task(&mut self, task: &Task) -> Result<()> {
        self.tasks.insert(task.id, task.clone());
        Ok(())
    }
    fn delete_task(&mut self, id: Uuid) -> Result<()> {
        self.tasks.remove(&id);
        Ok(())
    }
    fn find_tasks_by_prefix(&self, prefix: &str) -> Result<Vec<Task>> {
        Ok(self.tasks.values()
            .filter(|t| t.id.to_string().starts_with(prefix))
            .cloned().collect())
    }
    fn get_project(&self, path: &str) -> Result<Option<Project>> {
        Ok(self.projects.get(path).cloned())
    }
    fn list_projects(&self) -> Result<Vec<Project>> {
        Ok(self.projects.values().cloned().collect())
    }
    fn save_project(&mut self, project: &Project) -> Result<()> {
        self.projects.insert(project.path.clone(), project.clone());
        Ok(())
    }
    fn get_state(&self) -> Result<GlobalState> { Ok(self.state.clone()) }
    fn save_state(&mut self, state: &GlobalState) -> Result<()> {
        self.state = state.clone();
        Ok(())
    }
}
```

**`MemVcs`** records commits so tests can assert that the right git commits were made,
and can be told to simulate a conflict on pull:

```rust
// next-test-utils/src/mem_vcs.rs
#[derive(Default)]
pub struct MemVcs {
    pub commits: Vec<String>,          // commit messages in order
    pub pull_result: PullResult,       // configurable; default Clean
}

impl VcsBackend for MemVcs {
    fn commit(&mut self, _paths: &[PathBuf], message: &str) -> Result<()> {
        self.commits.push(message.to_string());
        Ok(())
    }
    fn pull(&self) -> Result<PullResult> { Ok(self.pull_result.clone()) }
    fn push(&self) -> Result<()> { Ok(()) }
    fn head_hash(&self) -> Result<String> { Ok("test-head".into()) }
}
```

### Unit tests (in `next`)

Tests import `next-test-utils` and build an `AppContext` directly:

```rust
// next/tests/list.rs
use next_test_utils::{MemStore, MemVcs};

#[test]
fn overdue_task_ranks_first() {
    let mut store = MemStore::default();
    store.save_task(&task_overdue()).unwrap();
    store.save_task(&task_due_tomorrow()).unwrap();

    let ctx = AppContext { store: Box::new(store), vcs: Box::new(MemVcs::default()), config: Config::default() };
    let output = next::commands::list::list(&ctx, FilterSet::default()).unwrap();
    assert_eq!(output.tasks[0].task.title, "overdue task");
}
```

### Integration tests (in `next-storage`)

`next-storage` tests use real temporary directories (actual TOML files, actual git repo,
actual SQLite database) to verify the full persistence round-trip:

```rust
// next-storage/tests/repo_store.rs
#[test]
fn round_trip_task() {
    let dir = tempfile::tempdir().unwrap();
    init_bare_repo(&dir);                    // git init + initial commit
    let (mut store, _vcs) = next_storage::open(dir.path().to_path_buf()).unwrap();
    let task = Task::new("Buy milk");
    store.save_task(&task).unwrap();
    let loaded = store.get_task(task.id).unwrap();
    assert_eq!(task.title, loaded.title);
}
```

### CLI tests (in `next-cli`)

CLI tests call `commands::*.run(args, ctx)` with a `MemStore`/`MemVcs` context, never
spawning a subprocess. They verify argument parsing and the shape of `CommandOutput`.

---

## External crate allocation

| Crate | Used in |
|-------|---------|
| `clap` | `next-cli` only |
| `rusqlite` | `next-storage` only |
| `git2` | `next-storage` only |
| `toml`, `serde` | `next-storage`, `next` (domain types need serde for JSON output) |
| `serde_json` | `next` (CommandOutput::Json) |
| `chrono` | `next` (domain types) |
| `interim` | `next` (date_parse module) |
| `rrule` | `next` (recurrence module) |
| `uuid` | `next` (Task::id) |
| `reqwest` | `next` (Forgejo + iCal URL fetch; blocking feature) |
| `icalendar` | `next` (iCal import/export) |
| `mockall` | `next` dev-dependencies |
| `dialoguer` | `next` (ReviewUi default impl) or `next-cli` |
| `owo-colors` | `next-cli` only (rendering concern) |
| `dirs` | `next-cli` only (XDG path resolution) |
| `anyhow`, `thiserror` | `next`, `next-storage` |
| `tempfile` | dev-dependencies in `next-storage`, `next-cli` |
| *(no mockall)* | stubs live in `next-test-utils`; no macro-generated mocks needed |
