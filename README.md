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

Every active task is a `.toml` file inside a `tasks/` directory at the repository root.
`git` is the transport layer — `next sync` runs pull then push. The TOML files are the
single source of truth.

`next` also maintains an SQLite database (`.next.db`) as a read cache. It is reconciled
incrementally whenever the git HEAD changes (e.g. after a pull), so it is always
consistent with the TOML files. The cache is stamped with the writing binary's version
and its table-schema version; when either differs from the running binary (an upgrade,
or a cache left by a different build), the next command rebuilds it from the TOML source
of truth automatically — so a version change can never surface stale or empty results.
Add it to `.gitignore`; `next init` does this for you.

Old closed tasks are **archived** automatically (at most once a day, during sync): they
move out of `tasks/` into month-keyed segment files under `archive/`, keeping the
working tree and git index small no matter how much history accumulates. Archived
tasks stay visible via `next list --archived` and still resolve by id or slug; editing
one brings it back automatically. Thresholds live in the committed
`config/archive.toml` (defaults: archive after 180 days; optional cold-tier pruning
off). See REQUIREMENTS.md §2.3 for the full lifecycle.

Machine-local state (the per-tag state and the active users) is stored
outside the repository in `$XDG_STATE_HOME/next/<repo-hash>/state.toml` so it
is never committed or synced.

```
my-tasks/
  .git/
  .gitignore                   # contains ".next.db"
  tasks/
    call-dentist-a1b2c3d4.toml
    water-plants.toml          # task with slug "water-plants"
  archive/
    2025/
      10-001.toml              # archived tasks completed in 2025-10 (≤1000 per segment)
  tags/
    __context__work.toml       # tag description for @work  (@ → __context__)
    __context__home/
      kitchen.toml             # tag description for @home/kitchen
  config/
    scoring.toml               # committed scoring weights (seeded by `next init`)
    archive.toml               # committed archive policy (optional; absent = defaults)
  .next.db                     # SQLite read cache — not committed

~/.local/state/next/<repo-hash>/
  state.toml                   # machine-local: per-tag state, active users
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

## Recurrence

A task recurs either on a **schedule** — an RFC 5545 RRULE, so the dates are fixed
calendar facts — or a fixed number of days **after completion**. `next done` marks the
instance done and creates the next one in the same git commit.

```sh
next add "Daily standup" --recur-schedule "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"
next add "Water plants" --recur-completion 7
```

A **snap** rounds the computed date to a boundary — a weekday, a workday, a day of the
month. On its own a snap only ever moves the date *later*, and by however far the next
boundary happens to be, so a completion-based task finished one day late has its cycle
stretched by a whole period:

```sh
# Complete this on the 2nd rather than the 1st and the next one is due 60 days later,
# not 30 — the 1st has gone past, so the snap jumps to the month after.
next add "Pay rent" --recur-completion 30 --recur-snap dom:1
```

`--recur-snap-leeway` bounds that movement, in days, and makes it two-sided. The date
moves to a boundary only if one is close enough; otherwise it keeps the interval it was
given:

```sh
next add "Pay rent" --slug rent --due 2026-06-01 \
  --recur-completion 30 --recur-snap dom:1 --recur-snap-leeway 3
```

Now completing on the 2nd pulls back to the 1st of next month instead of jumping the
month, and completing on the 15th leaves the date on the 15th — off the boundary, but on
cadence. `N` sets both directions; `BACK,FORWARD` sets them independently (`5,0` never
pushes a date later) and `BACK,*` leaves the forward direction unbounded. Leaving the flag
off keeps the old forward-only behaviour exactly, so no existing task changes date.

See [`CLI.md`](CLI.md#recurrence) for the RRULE surface, the snap values, the full leeway
rule and its validation errors.

---

## Tag system

All labels on a task are tags, and every tag behaves the same way. The prefix is a
**naming convention** that says what a tag is for — it does not change how the tag
filters:

| Prefix | Kind | Example | What it usually names |
|--------|------|---------|-----------------------|
| `@` | Context | `@home`, `@work` | A working environment |
| `#` | Resource | `#printer`, `#vacation` | Something you need available |
| *(none)* | Freeform | `python`, `urgent` | Anything else |

Tags nest with `/` (`@work/frontend`, `#office/printer`), and a filter or state on a
parent applies to every descendant.

### Tag state

Each tag is **required**, **excluded**, or **accepted**, and the same three states apply
to every kind:

```sh
next tag require @home           # only @home tasks (and nothing untagged)
next tag require @home @errands  # either one — requiring several is a disjunction
next tag exclude '#printer'      # the printer is broken; hide its tasks
next tag clear-state             # back to showing everything
```

While anything is required, only tasks carrying a required tag are listed. An excluded
tag hides a task even if another of its tags is required. State is inherited by
descendants, and `next tag accept <tag>` pins a tag to neither, so it can opt out of
its parent's:

```sh
next tag exclude @home           # not doing home tasks…
next tag accept @home/kitchen    # …except in the kitchen
```

`accept` is not the same as `clear-state`: accepting *pins* the tag, which is what stops
it inheriting from its parent, while clearing removes the entry so inheritance resumes.

`next tag` shows the current state at the top of its listing.

### Tag descriptions

Any tag can carry a human-readable description. These descriptions are stored as
individual TOML files under `tags/` in the repository and are committed to git, making
them visible to all machines. They appear in `next tag` output and serve as structured
metadata for AI agents reading the repository.

```sh
next tag describe @work "Tasks at the standing desk — laptop required"
next tag describe @home "Home tasks: kitchen, garden, errands"
next tag describe #printer "Office laser printer, 2nd floor"
next tag                        # list all tags with their descriptions
```

`next tag` is the single command for everything about tags: the state
(`require`, `exclude`, `accept`, `clear-state`), the metadata (`describe`, `set-url`,
`set-priority`, `set-no-time-urgency`, `data`, `show`, and the matching `clear-*`
subcommands), and `rename`.

### Renaming a tag

`next tag rename <old> <new>` moves a tag everywhere it is recorded: every active task,
every archived task (including segments already pruned to the cold tier), the tag's
metadata file, and the machine-local tag state — all in one commit.

```sh
next tag rename @ai/task-manager @ai/next   # @ai/task-manager/* moves along with it
next tag rename py python --merge           # fold py into an existing python tag
```

Renaming is hierarchical: everything nested under the tag moves with it. A tag's kind
cannot change (`@work` cannot become `#work`), and an existing destination is refused
unless `--merge` is given — with `--merge`, a task carrying both tags keeps one copy and
the destination's metadata wins.

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

Closed tasks (`done` or `cancelled`) always score exactly 0: a score is advice about what to
work on next, so it does not apply to a finished task. Since nothing breaks the tie, a
`--closed` listing keeps the completion order it comes out of the store with (most recently
completed first), and a `--all` listing puts live work above the closed tail.

Tags can suppress time-based urgency entirely with `next tag set-no-time-urgency <tag>` — useful for wishlist or someday tags where age and deadlines should not drive priority.

---

## Filtering

All list commands (`list`, `next`, `tree`, `forecast`) take a filter
expression. The trailing arguments are joined into one query, so short filters
need no quoting — and the MCP `filter` parameter takes the identical string.

> **A bare word searches; it is not a tag.** `next list bug` looks for "bug" in
> the title, description, notes and url. For tasks *tagged* `bug`, write
> `next list +bug`.

The expression is the trailing part of the command line and every flag comes
before it — `next list --all +@work`, not `next list +@work --all`, which is
rejected. Trailing arguments are taken verbatim so `-bug` keeps working as an
exclusion, which leaves no way to recognise a flag among them.

| Form | Example | Meaning |
|------|---------|---------|
| `<word>` | `bug`, `"cold tier"`, `arch*` | Search the text. Whole words; a phrase is ordered; `*` matches by prefix |
| `+<tag>` / `-<tag>` | `+@home`, `-@work` | Must / must not have the tag, nested tags included |
| `<field>:<value>` | `status:open`, `due<+7d`, `data.k:v` | Field predicates, with `<` `<=` `>` `>=` and `a..b` ranges on ordered fields |
| `id:<id>` | `id:a1b2c3d4` | Task has this id; matches by prefix, so the short id a listing prints is enough |
| `has:` / `no:` / `is:` | `has:due`, `is:overdue` | Field presence and named predicates |
| `and` `or` `not` `( )` | `+@work and not is:blocked` | Booleans; adjacency means `and` |
| `parent:<slug>` | `parent:launch-blog` | Scope to a project subtree |
| `--future` / `--all` | | Include future-start tasks / disable implicit filtering |
| `--count` / `--format` | | Print the match count / choose `table` or `json` |
| `--explain` / `--fields` | | Show what the query parsed to / project the JSON output (JSON only) |

The implicit gate runs before the expression, so a query for something it
already hides comes back empty: `next list status:done` and `next list is:closed`
match nothing until you add `--all`. The listing prints a hint saying so.

See [CLI.md](CLI.md#filter-syntax) for the full reference.

---

## Command summary

| Command | Description |
|---------|-------------|
| `next init` | Initialise a task repository in the current directory |
| `next add` | Add a task |
| `next list [-n N] [--page N] [--archived]` | List tasks sorted by urgency score; paginated (`--page`/`--page-size`, `-n`/`--limit` caps output, `list_limit` applies to the archive too); `--closed` shows done/cancelled, `--archived` lists the archive; `--count` prints the total, `--explain` shows how the query parsed |
| `next next [N]` | Show top N highest-scored tasks (default 10) |
| `next show <id>` | Full details of a single task |
| `next tree` | Show all tasks in a parent-child tree; takes the same filter, keeping the ancestors of a match so the tree still has branches |
| `next start <id>` | Mark as started (in-progress); logs a time entry |
| `next stop <id>` | Stop a started task (returns to open); logs a time entry |
| `next done <id> [--completed-at <date>]` | Mark done; triggers recurrence if applicable |
| `next cancel <id>` | Mark cancelled |
| `next edit <id>` | Modify fields on an existing task |
| `next delete <id>` | Permanently remove a task |
| `next move <id>` | Change parent task |
| `next open <id>` | Open the task's URL in the browser |
| `next data set/unset/get` | Manage arbitrary key-value data on a task |
| `next tag [rename/describe/set-priority/set-no-time-urgency/…]` | List tags; rename a tag; manage tag metadata |
| `next tag [require/exclude/accept/clear-state]` | Set which tags are required, excluded, or pinned to accepted |
| `next user [set/clear/list]` | Manage user filter |
| `next plugin [register/watch/unwatch/unregister/set-sync/set-interval/enable/disable/list]` | Manage export plugins and their periodic syncs (see [Plugins](#plugins)) |
| `next forecast` | Upcoming due dates grouped by time, including projected occurrences of both recurrence modes over the horizon |
| `next sync` | Pull from remote, auto-archive if due, push local commits, run due plugin syncs |
| `next maintenance archive` | Move old closed tasks into archive segments now |
| `next maintenance rebuild-cache` | Drop and rebuild the local `.next.db` read cache |
| `next config [get/set]` | Read or write a value in the machine-local `config.toml` |
Task IDs accept a full UUID, a slug, or any unambiguous 4+ character hex prefix.
All commands support `--json` for pipe-friendly output, and the task-listing ones
take `--fields id,title,due` to project it down to what you actually read — the
default returns every field of every task, notes included. Running `next` with no
subcommand is the same as `next list`.

Sync has two orthogonal capabilities, each with a config key and a paired override flag
pair:

- **autopull** (`sync.autopull`, default on; `--autopull` / `--no-autopull`) — before every
  command except `next sync`, a best-effort staleness pull: if the last pull is older than
  `staleness_secs` (default 1 hour), it pulls from the remote so results reflect other
  machines' changes.
- **autopush** (`sync.autopush`, default off; `--autopush` / `--no-autopush`) — a push after
  every successful mutation command.

The master flags `--autosync` / `--no-autosync` toggle **both** at once for one invocation;
`--offline` is an alias for `--no-autosync` (no network I/O at all). The master and granular
flags are mutually exclusive. `next sync` always pulls and pushes and refuses to run with any
disabling flag.

### Logging

`next` emits diagnostic logs through `tracing`. By default only errors are shown. Raise the
verbosity for one invocation with the global `--log-level` flag (`error`, `warn`, `info`,
`debug`, or `trace`):

```sh
next --log-level debug add "buy milk"
```

The flag sets the default log level; any `RUST_LOG` per-target directives still apply on top,
so you can raise the floor with `--log-level debug` while silencing a noisy module via
`RUST_LOG=next::core::sync=warn`.

---

## Configuration

`next` stores tasks locally as TOML files in a git repository.

**Machine-local settings** live at `$XDG_CONFIG_HOME/next/config.toml` (CLI only):

```toml
repository            = "/home/alice/tasks"  # use next from any directory

list_limit            = 20                   # cap `next list` output, archive included (same as -n 20)
forecast_horizon_days = 90                   # days ahead shown by `next forecast`
next_count            = 10                   # tasks shown by `next next`

[sync]
git_subprocess        = true                 # use `git` subprocess instead of libgit2
autopull              = true                 # staleness pull before commands (default); --no-autopull to bypass
autopush              = false                # push after each mutation (default off); --autopush to enable
staleness_secs        = 3600                 # re-pull after this many seconds (default: 1 hour)
pull_timeout_secs     = 10                   # autopull timeout (stored; not yet enforced)
plugin_sync_default_secs = 86400             # system-default plugin sync interval (see Plugins)
```

Values can also be read and written from the command line with
`next config get [<key>]` / `next config set <key> <value>`
(e.g. `next config set sync.autopull false`).

> **Migration:** the old top-level `autosync` key is now `[sync] autopush`; `[sync]
> pull_before_query` is renamed to `[sync] autopull` (the old name is still accepted as
> an alias); `[sync] offline` is removed (use `--offline` or set `autopull`/`autopush` to
> `false`). The `--no-sync` flag is gone — use `--offline`.

**Scoring weights** live *in the repository* at `config/scoring.toml`, committed to git
and synced. `next init` seeds it with the defaults. Because it is part of the repo, the
CLI, the MCP server, and plugins all score tasks the same way. Omitted fields fall back to
the built-in defaults, so you can keep just the weights you change:

```toml
# config/scoring.toml
priority_high = 3.0   # only override what you want to change
age_max       = 3.0
```

Remote access is provided via MCP — see the MCP server section below.

---

## Terminal UI (`next-tui`)

`next-tui` is an optional full-screen terminal UI built on [ratatui](https://ratatui.rs/).
It links the core library directly (no shelling out to `next`) and edits the repository
through the same transactions and git commits as the CLI, so it is safe to run alongside
the CLI and the MCP server. It offers list / tree / forecast views, a full edit modal,
per-task actions, a tag/user state panel, and background sync.

```sh
cargo build --release --features tui --bin next-tui
next-tui                     # or: next-tui --repo <path> --config <path>
```

It loads its own `tui.toml`, falling back to the CLI's `config.toml`, then defaults.
See [`TUI.md`](TUI.md) for the full keymap and view reference.

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

Configuration comes from a TOML config file and environment variables, with env vars
taking precedence: **env var > config file > built-in default**. The config file lives
at `/data/config/next-mcp/config.toml` inside the container (the image sets
`XDG_CONFIG_HOME=/data/config`); override the path with `next-mcp --config <path>` or
`NEXT_MCP_CONFIG`. See `quadlets/next-mcp.config.toml.example` for the full schema
(`bearer_token`, `webhook_token`, `repo_path`, `bind_addr`, plus `[git]` and `[sync]`
tables mirroring the variables below).

**Secrets** (`bearer_token`, `webhook_token`, `git.token`) each accept exactly one of
three forms, so the config file can stay non-secret: an inline value, `<name>_file` (a
path such as a podman secret at `/run/secrets/…`), or `<name>_env` (the name of another
env var). Setting more than one form for the same secret is an error; the matching
`NEXT_*` env var still overrides all of them.

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `NEXT_BEARER_TOKEN` | ✓ (or file) | — | MCP client auth token |
| `NEXT_MCP_CONFIG` | | `/data/config/next-mcp/config.toml` | Path of the TOML config file (`--config` flag wins; `NEXT_CONFIG` is a deprecated alias) |
| `NEXT_GIT_URL` | first start | — | HTTPS URL to clone the tasks repo |
| `NEXT_GIT_USER` | | — | Git username (or embed in URL as `https://user:token@…`) |
| `NEXT_GIT_TOKEN` | | — | Git password/token |
| `NEXT_GIT_AUTHOR_NAME` | | `next-mcp` | Committer name when no git identity is configured |
| `NEXT_GIT_AUTHOR_EMAIL` | | `next-mcp@unknown` | Committer email when no git identity is configured |
| `NEXT_GIT_PARTIAL_CLONE` | | `1` | First-start clone tries `--filter=blob:none`; `0` forces the built-in full clone (no git binary needed) |
| `NEXT_REPO_PATH` | | `/data/tasks` | Local path for the tasks repository |
| `NEXT_BIND_ADDR` | | `0.0.0.0:3000` | Listen address |
| `NEXT_WEBHOOK_TOKEN` | | — | If set, enables `POST /webhook/sync` with this token |
| `NEXT_SYNC_INTERVAL` | | `86400` | Periodic pull+push interval in seconds; `0` disables |
| `NEXT_DEFERRED_SYNC_DELAY_SECS` | | `30` | Seconds before deferred sync fires after `autosync=false` |
| `NEXT_AUTOPULL` | | `true` | Staleness pull before task-touching tools; `false`/`0`/`no` disables (old name `NEXT_PULL_BEFORE_QUERY` still read as a fallback) |
| `NEXT_STALENESS_SECS` | | `3600` | How long the local copy stays fresh after a pull |
| `NEXT_PULL_TIMEOUT_SECS` | | `10` | Pre-query pull timeout (stored; not yet enforced) |

Credentials embedded in `NEXT_GIT_URL` are stripped before logging.

```sh
NEXT_BEARER_TOKEN=secret \
NEXT_GIT_URL=https://git.example.com/user/tasks.git \
NEXT_GIT_TOKEN=my-pat \
next-mcp
```

### Quadlet (Podman)

Copy `quadlets/next-mcp.container` to `~/.config/containers/systemd/`, then place a
`config.toml` in the `next-config` volume at `next-mcp/config.toml` — use
`quadlets/next-mcp.config.toml.example` as a starting point. Provision secrets as
podman secrets and reference them from the config with the `*_file` forms:

```sh
podman secret create next_bearer_token -   # paste token, then Ctrl-D
podman secret create next_git_token -
```

```toml
# /data/config/next-mcp/config.toml
bearer_token_file = "/run/secrets/next_bearer_token"
[git]
url        = "https://git.example.com/user/tasks.git"
user       = "user"
token_file = "/run/secrets/next_git_token"
```

The `Secret=` lines in the quadlet mount those at `/run/secrets/…`. Environment
variables remain a permanent top-precedence override (env > config > default), so an
existing `EnvironmentFile=` deployment keeps working — just uncomment that line in the
quadlet.

Three named volumes are used — Podman creates them automatically on first start:

| Volume | Mount | Contents |
|--------|-------|----------|
| `next-tasks` | `/data/tasks` | Cloned tasks git repository |
| `next-state` | `/data/state` | Machine-local state (per-tag state, user filter) |
| `next-config` | `/data/config` | `config.toml` at `next-mcp/config.toml` (see above) |

Then:

```sh
systemctl --user daemon-reload
systemctl --user start next-mcp
```

### MCP tools (15)

| Tool | R/M | Description |
|------|-----|-------------|
| `list_tasks` | R | List scored tasks; accepts one `filter` expression string (same syntax as the CLI), `page`/`page_size` pagination, `fields` projection, and `archived: true` for the archive |
| `get_task` | R | Full details of one task + direct children + score breakdown; `fields` projects the response, and the score and its breakdown then come back only when named |
| `add_task` | M | Create a task (inherits the required `@context` tags if the task has none) |
| `update_task` | M | Edit fields or transition state (start/stop/done/cancel/move); `done` accepts `completed_at` |
| `delete_task` | M | Permanently remove a task |
| `sync` | M | Pull then push (`push_only`/`pull_only` optional); fails fast if sync already in progress |
| `get_diff` | R | Working-tree diff (git status + diff HEAD) for inspecting conflicts/uncommitted changes |
| `force_sync` | M | Fetch + hard-reset to FETCH_HEAD, discarding local changes; recovery from stuck conflicts |
| `get_state` | R | The per-tag state map and the active users |
| `set_tag_state` | M | Set tags to `required`/`excluded`/`accepted`, or `clear` their entry |
| `set_user_filter` | M | Replace active user filter |
| `manage_tag` | R/M | Tag rename plus metadata CRUD (list/show/rename/describe/set_priority/set_no_time_urgency/…) |
| `manage_task_data` | R/M | Task data key-value pairs (get/list/set/unset) |
| `get_forecast` | R | Upcoming due dates within a configurable horizon; accepts `context` override |

All mutation tools (M) accept an `autosync: bool` parameter (default `true`):

- **`autosync: true`** — sync runs inline before the response is returned. Sync errors are logged but do not fail the tool call.
- **`autosync: false`** — mutation returns immediately; a deferred sync fires `NEXT_DEFERRED_SYNC_DELAY_SECS` seconds after the last mutation in a batch. Useful when making many changes and calling `sync` explicitly at the end.

At most one sync runs at a time — the `sync` tool and webhook return an error immediately if a sync is already in progress rather than queuing.

Before each task-touching tool call the server also runs a best-effort staleness pull
(the same autopull as the CLI), controlled by `NEXT_AUTOPULL` (old name
`NEXT_PULL_BEFORE_QUERY` still read as a fallback) / `NEXT_STALENESS_SECS`, so responses
reflect other machines' pushes.

**Slug format**: letters, digits, `-` and `_` only (e.g. `water-plants`, `work_infra`).

### Server instructions

The `initialize` response includes an `instructions` string (a standard MCP field) that
clients MAY inject into the model's system prompt. It carries the tagging conventions
(`@context`, `#resource`, freeform as naming conventions; the three tag states; `/`
hierarchy; tag priority / no-time-urgency metadata) plus a connect-time snapshot of the
current tag state and the catalog of known tags with their descriptions. This pushes tag
knowledge onto the AI up front so it reuses existing tags and respects the tag state
without having to call `manage_tag list` / `get_state` first. The snapshot is taken at
connection time and refreshes on reconnect.

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
next plugin set-sync <name> [--default-interval S] -- <program> [args…]
                                                   # define the periodic-sync (import) command
next plugin set-interval <name> <secs>|--clear     # user override of the sync interval
next plugin enable <name> / disable <name>         # toggle the periodic sync
next plugin list
```

A plugin typically registers itself: after importing an external item as a task, it runs
`next plugin watch <name> <task-id>` so it learns about later changes. Registrations are
**machine-local** — stored in the `[[plugin]]` section of `state.toml` under
`$XDG_STATE_HOME/next/<hash>/`, never committed to git (plugin binaries are
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

### Periodic plugin sync

Besides the per-task export hook, a plugin can register a **periodic sync** (import)
command with `next plugin set-sync`. After every successful `next sync`, each enabled
plugin whose sync is *due* has its command run to completion (cwd = repo root, with
`NEXT_REPO` and the `NEXT_PLUGIN_ORIGIN` loop guard set). Only a successful exit
records `last_sync`, so failures retry on the next sync. The interval resolves as
**user override → plugin default → system default** — i.e. `set-interval`, then
`set-sync --default-interval`, then `plugin_sync_default_secs` in `config.toml`
(default 86400 = daily). This replaces a cron entry for `next-forgejo sync` and
similar importers.

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

Run `sync` periodically to keep imports current — either via cron / systemd timer, or
by registering it as a periodic plugin sync so it runs after each `next sync`:

```sh
next plugin set-sync next-forgejo -- next-forgejo sync
```

---

## Cargo features

`next` is one crate with feature-gated binaries on top of a feature-free core library:

| Feature | Default | Builds |
|---------|---------|--------|
| `cli` | ✓ | the `next` CLI binary (pulls in `clap`) |
| `mcp` | | the `next-mcp` server (`--features mcp`) |
| `forgejo` | | the `next-forgejo` plugin (`--features forgejo`) |
| `tui` | | the `next-tui` terminal UI (`--features tui`) |

```sh
cargo build                          # the next CLI (default)
cargo build --no-default-features    # core library only — no CLI, no clap
cargo build --features mcp           # + the MCP server
cargo build --features forgejo       # + the Forgejo plugin
cargo build --features tui           # + the terminal UI
```

With `--no-default-features` the crate is just the core library under `src/core/` (`domain`,
`storage`, `store`, `plugin`, `config`, `scoring`, `service`, `task_repository`, …) that other
crates can link without the CLI or its dependencies. Logic shared between the CLI, MCP server,
and plugins lives in the `core` module so the feature modules depend only on the core.
(`AppContext` is part of the `cli` feature, not the no-features core.)

---

## See also

- `REQUIREMENTS.md` — functional requirements
- `ARCHITECTURE.md` — internal design and module layout
- `CLI.md` — full command reference
- `TUI.md` — terminal UI keymap and view reference
