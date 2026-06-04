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

Machine-local state (active contexts, active users, resource availability) is stored
outside the repository in `$XDG_STATE_HOME/task-manager/<repo-hash>/state.toml` so it
is never committed or synced.

```
my-tasks/
  .git/
  .gitignore                   # contains ".next.db"
  tasks/
    call-dentist-a1b2c3d4.toml
    water-plants.toml          # task with slug "water-plants"
  tags/
    __context__work.toml       # tag description for @work  (@ → __context__)
    __context__home/
      kitchen.toml             # tag description for @home/kitchen
  next.log                     # append-only command log (rotated at 1 MB)
  .next.db                     # SQLite read cache — not committed

~/.local/state/task-manager/<repo-hash>/
  state.toml                   # machine-local: active contexts, users, resource availability
```

---

## Projects and subtasks

Any task can have subtasks — no special tag or type is required. Subtasks attach via
`--parent`. A parent task is hidden from the default scored list until all its direct
children are resolved.

```sh
next add "Launch blog" --slug launch-blog
next add "Write first post" --parent launch-blog
next add "Set up hosting" --parent launch-blog
next tree                      # show all tasks in a parent-child tree
next list parent:launch-blog   # list tasks within this project
next show launch-blog          # show full details for a single task
```

---

## Tag system

All labels on a task are tags. Prefix conventions give some tags special meaning:

| Prefix | Kind | Example | Effect |
|--------|------|---------|--------|
| `@` | Context | `@home`, `@work` | Hidden when a different context is active |
| `#` | Resource | `#printer`, `#vacation` | Hidden when resource is unavailable |
| *(none)* | Freeform | `python`, `urgent` | No implicit filter |

Tasks with no `@` tag are always shown regardless of the active context.

```sh
next context set @home          # global context filter
next context clear              # show all contexts
next context exclude @work      # always hide @work tasks
next context clear-excluded     # remove exclusions

next resource set #printer off  # hide printer tasks
next resource set #printer on   # show them again
```

### Tag descriptions

Any tag (context, resource, or freeform) can carry a human-readable description. These
descriptions are stored as individual TOML files under `tags/` in the repository and are
committed to git, making them visible to all machines. They appear in `next tag`,
`next context`, and `next resource` output and serve as structured metadata for AI agents
reading the repository.

```sh
next tag describe @work "Tasks at the standing desk — laptop required"
next context describe @home "Home tasks: kitchen, garden, errands"
next resource describe #printer "Office laser printer, 2nd floor"
next tag                        # list all tags with their descriptions
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
```

---

## Urgency scoring

Every visible task receives a numeric score used for ranking:

```
score = due_factor + priority_factor + project_factor + age_factor + score_adjustment
```

- **Due factor** — rises sharply as the deadline approaches; peaks when overdue (zeroed by `no_time_urgency`)
- **Priority** — `high` (+2), `medium` (+1), `low` (0)
- **Project factor** — parent task's priority offsets the score (+0.5 / 0.0 / −0.5)
- **Age** — older tasks float up; capped at +2; zeroed when `long_term = true` or `no_time_urgency`
- **Tag factor** — each tag with explicit `priority` metadata adds an offset (`high` +1.0, `low` −1.0); applied to both the task's own tags and the parent's tags
- **Started bonus** — flat +4.0 when `status = started`; moves in-progress tasks above open peers
- **Adjustment** — manual boost/penalty via `--adjust`

Tags can suppress time-based urgency entirely with `next tag set-no-time-urgency <tag>` — useful for wishlist or someday tags where age and deadlines should not drive priority.

---

## Filter tokens

All list commands accept filter tokens in any order:

| Token | Example | Meaning |
|-------|---------|---------|
| `+<tag>` | `+@home`, `+python` | Task must have this tag |
| `-<tag>` | `-@work` | Task must not have this tag |
| `parent:<slug>` | `parent:launch-blog` | Task is a descendant of (or is) the task with this slug |
| `context:<@tag>` | `context:@home` | Override active context for this query |
| `user:<name>` | `user:alice` | Override user filter for this query |
| `--future` | | Include tasks with a future `start` date |
| `--all` | | Disable all implicit filtering |

---

## Command summary

| Command | Description |
|---------|-------------|
| `next init` | Initialise a task repository in the current directory |
| `next add` | Add a task |
| `next list [-n N]` | List tasks sorted by urgency score; `-n`/`--limit` caps output |
| `next next [N]` | Show top N highest-scored tasks (default 10) |
| `next show <id>` | Full details of a single task |
| `next tree` | Show all tasks in a parent-child tree |
| `next start <id>` | Mark as started (in-progress); logs a time entry |
| `next stop <id>` | Stop a started task (returns to open); logs a time entry |
| `next done <id> [--completed-at <date>]` | Mark done; triggers recurrence if applicable |
| `next cancel <id>` | Mark cancelled |
| `next edit <id>` | Modify fields on an existing task |
| `next delete <id>` | Permanently remove a task |
| `next move <id>` | Change parent task |
| `next open <id>` | Open the task's URL in the browser |
| `next data set/unset/get` | Manage arbitrary key-value data on a task |
| `next tag [describe/set-priority/set-no-time-urgency/…]` | List tags; manage tag metadata |
| `next context [set/clear/exclude/clear-excluded]` | Manage global context filter (include and exclude lists) |
| `next resource [set]` | Manage resource availability |
| `next user [set/clear/list]` | Manage user filter |
| `next plugin [register/watch/unwatch/unregister/list]` | Manage export plugins (see [Plugins](#plugins)) |
| `next forecast` | Show upcoming due dates grouped by time |
| `next sync` | Pull from remote, push local commits |
Task IDs accept a full UUID, a slug, or any unambiguous 4+ character hex prefix.
All commands support `--json` for pipe-friendly output.

The `--autosync` global flag (or `autosync = true` in the config file) automatically
runs `next sync` after every mutation command.

---

## Configuration

`next` stores tasks locally as TOML files in a git repository. The config file lives at
`$XDG_CONFIG_HOME/task-manager/config.toml`:

```toml
repository = "/home/alice/tasks"  # use next from any directory

autosync   = true                 # sync automatically after each mutation
list_limit = 20                   # cap `next list` output (same as -n 20)
```

Remote access is provided via MCP — see the MCP server section below.

---

## MCP server (`next-mcp`)

`next-mcp` is an optional HTTP server that exposes the full task management API over the
[Model Context Protocol](https://spec.modelcontextprotocol.io/) (MCP). It is designed to
run as a Podman Quadlet container so that AI assistants (Claude, etc.) can manage tasks
remotely.

### Building

```sh
# Binary only
cargo build --release --features mcp --bin next-mcp

# Container image (published at forgejo.victorsavu.eu/victor/next-mcp)
podman build -f Containerfile -t localhost/next-mcp:latest .
```

### Configuration

All configuration is passed via environment variables — no config file.

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `NEXT_BEARER_TOKEN` | ✓ | — | MCP client auth token |
| `NEXT_GIT_URL` | first start | — | HTTPS URL to clone the tasks repo |
| `NEXT_GIT_USER` | | — | Git username (or embed in URL as `https://user:token@…`) |
| `NEXT_GIT_TOKEN` | | — | Git password/token |
| `NEXT_REPO_PATH` | | `/data/tasks` | Local path for the tasks repository |
| `NEXT_BIND_ADDR` | | `0.0.0.0:3000` | Listen address |
| `NEXT_WEBHOOK_TOKEN` | | — | If set, enables `POST /webhook/sync` with this token |
| `NEXT_SYNC_INTERVAL` | | `86400` | Periodic pull+push interval in seconds; `0` disables |
| `NEXT_DEFERRED_SYNC_DELAY_SECS` | | `30` | Seconds before deferred sync fires after `autosync=false` |

Credentials embedded in `NEXT_GIT_URL` are stripped before logging.

```sh
NEXT_BEARER_TOKEN=secret \
NEXT_GIT_URL=https://git.example.com/user/tasks.git \
NEXT_GIT_TOKEN=my-pat \
next-mcp
```

### Quadlet (Podman)

Copy `quadlets/next-mcp.container` to `~/.config/containers/systemd/` and create
`~/.config/next-mcp/env` (chmod 600) — use `quadlets/next-mcp.env.example` as a
starting point:

```
NEXT_BEARER_TOKEN=…
NEXT_WEBHOOK_TOKEN=…          # optional; enables POST /webhook/sync
NEXT_GIT_URL=https://git.example.com/user/tasks.git
NEXT_GIT_USER=user
NEXT_GIT_TOKEN=…
NEXT_SYNC_INTERVAL=86400
```

Two named volumes are used — Podman creates them automatically on first start:

| Volume | Mount | Contents |
|--------|-------|----------|
| `next-tasks` | `/data/tasks` | Cloned tasks git repository |
| `next-state` | `/data/state` | Machine-local state (active context, user filter, resources) |

Then:

```sh
systemctl --user daemon-reload
systemctl --user start next-mcp
```

### MCP tools (13)

| Tool | R/M | Description |
|------|-----|-------------|
| `list_tasks` | R | List scored tasks; accepts filter tokens + `context` override |
| `get_task` | R | Full details of one task + direct children + score breakdown |
| `add_task` | M | Create a task (auto-applies active context if task has none) |
| `update_task` | M | Edit fields or transition state (start/stop/done/cancel/move); `done` accepts `completed_at` |
| `delete_task` | M | Permanently remove a task |
| `sync` | M | Pull then push; fails fast if sync already in progress |
| `get_state` | R | Active contexts, excluded contexts, users, resource availability |
| `set_context` | M | Replace active and/or excluded context filters |
| `set_resource` | M | Toggle resource availability |
| `set_user_filter` | M | Replace active user filter |
| `manage_tag` | R/M | Tag metadata CRUD (list/show/describe/set_priority/set_no_time_urgency/…) |
| `manage_task_data` | R/M | Task data key-value pairs (get/list/set/unset) |
| `get_forecast` | R | Upcoming due dates within a configurable horizon; accepts `context` override |

All mutation tools (M) accept an `autosync: bool` parameter (default `true`):

- **`autosync: true`** — sync runs inline before the response is returned. Sync errors are logged but do not fail the tool call.
- **`autosync: false`** — mutation returns immediately; a deferred sync fires `NEXT_DEFERRED_SYNC_DELAY_SECS` seconds after the last mutation in a batch. Useful when making many changes and calling `sync` explicitly at the end.

At most one sync runs at a time — the `sync` tool and webhook return an error immediately if a sync is already in progress rather than queuing.

**Slug format**: letters, digits, `-` and `_` only (e.g. `water-plants`, `work_infra`).

### Server instructions

The `initialize` response includes an `instructions` string (a standard MCP field) that
clients MAY inject into the model's system prompt. It carries the tagging conventions
(`@context`, `#resource`, freeform; `/` hierarchy; tag priority / no-time-urgency
metadata) plus a connect-time snapshot of the active context, unavailable resources, and
the catalog of known tags with their descriptions. This pushes tag knowledge onto the AI
up front so it reuses existing tags and respects the active context without having to call
`manage_tag list` / `get_state` first. The snapshot is taken at connection time and
refreshes on reconnect.

### Webhook

`POST /webhook/sync` triggers an immediate pull+push using `NEXT_WEBHOOK_TOKEN` for auth (strictly separate from the MCP bearer token — neither token is accepted on the other route). Wire it to your git host's push webhook to keep the container up to date when others push.

### Security notes

- Container runs as unprivileged user `next` (UID 1000)
- Request bodies are capped at 64 KB
- Bearer token comparison is constant-time and does not leak the expected token's length
- Embedded git credentials are stripped from `NEXT_GIT_URL` before any logging

---

## Plugins

Integrations (e.g. Forgejo, WebDAV/CalDAV) live outside the core as **external plugin
binaries**. A plugin subscribes to individual tasks and is notified whenever one of them
changes; it then does its work by linking the `next` library or calling the `next` CLI.

### Registering a plugin

```sh
next plugin register <name> -- <program> [args…]   # define/replace a plugin's command
next plugin watch   <name> <task>                  # notify <name> on any update to <task>
next plugin unwatch <name> <task>
next plugin unregister <name>
next plugin list
```

A plugin typically registers itself: after importing an external item as a task, it runs
`next plugin watch <name> <task-id>` so it learns about later changes. Registrations are
**machine-local** — stored in `plugins.toml` beside the state file under
`$XDG_STATE_HOME/task-manager/<hash>/`, never committed to git (plugin binaries are
per-machine).

### Notification contract

When a watched task is updated (any of add/start/stop/done/cancel/edit/move/delete/data),
`next` spawns the plugin's command **fire-and-forget**, after the repository lock is
released, with:

- **stdin**: a JSON event, also provided in `NEXT_PLUGIN_EVENT`:
  ```json
  { "event": "done", "task_id": "<uuid>", "repo": "<path>", "timestamp": "<rfc3339>" }
  ```
- **env**: `NEXT_REPO` (repo root), `NEXT_PLUGIN_EVENT` (the JSON above),
  `NEXT_PLUGIN_ORIGIN` (the plugin's own name).
- **cwd**: the repository root.

Delivery is best-effort (a missed event is reconciled on the plugin's next run); plugins
should be idempotent. **Loop guard:** a plugin is never notified of changes it caused
itself — `next` sets `NEXT_PLUGIN_ORIGIN` when spawning the plugin, and any `next`
mutations the plugin makes (which inherit that env) skip notifying that same plugin.

### Forgejo plugin

The first bundled plugin maps Forgejo repositories to `next` contexts. It ships in this
crate behind the `forgejo` Cargo feature (off by default, like `mcp`) as the
`next-forgejo` binary:

```sh
cargo build --release --features forgejo   # builds next-forgejo
```

Configure `~/.config/next-forgejo/config.toml`:

```toml
forgejo_url   = "https://forgejo.victorsavu.eu"
forgejo_token = "<api token>"
# next_repo = "/home/you/tasks"   # optional; else next's configured/default repo

[[map]]
repo    = "victor/task-manager"   # owner/repo on Forgejo
context = "@ai/task-manager"      # imported tasks get this context tag
```

Commands:

```sh
next-forgejo register      # register the export hook (sync also does this)
next-forgejo sync          # import issues + reconcile resolution (both ways)
next-forgejo sync --dry-run
next-forgejo hook          # internal: invoked by next's export hook
```

`sync` imports each **open** issue with no task yet (title, issue url, the mapped
`@context`, body → description), links them via `__forgejo-*` task data, and subscribes the
plugin so local resolution propagates. Resolution syncs **both ways** (close-only in v1):

- A Forgejo issue closed → the linked task is marked **done** on the next `sync`.
- A task resolved locally (`next done`/`cancel`) → the export hook closes the Forgejo issue
  in real time; `sync` also reconciles any resolution the hook missed.

Link data attributes on each task (`__forgejo-` prefix): `__forgejo-repo` (`owner/repo`),
`__forgejo-issue` (number), `__forgejo-url`, and `__forgejo-labels` (the issue's labels, as
a JSON array — kept as data rather than local tags for now). Reopening is manual in v1.

Run `sync` periodically (cron / systemd timer) to keep imports current.

---

## Cargo features

`next` is one crate with feature-gated binaries on top of a feature-free core library:

| Feature | Default | Builds |
|---------|---------|--------|
| `cli` | ✓ | the `next` CLI binary (pulls in `clap`) |
| `mcp` | | the `next-mcp` server (`--features mcp`) |
| `forgejo` | | the `next-forgejo` plugin (`--features forgejo`) |

```sh
cargo build                          # the next CLI (default)
cargo build --no-default-features    # core library only — no CLI, no clap
cargo build --features mcp           # + the MCP server
cargo build --features forgejo       # + the Forgejo plugin
```

With `--no-default-features` the crate is just the core (`domain`, `storage`, `store`,
`plugin`, `core`, `app_context`, `config`) that other crates can link without the CLI or
its dependencies. Logic shared between the CLI, MCP server, and plugins lives in the
`core` module so the feature modules depend only on the core.

---

## See also

- `REQUIREMENTS.md` — functional requirements
- `ARCHITECTURE.md` — internal design and module layout
- `CLI.md` — full command reference
