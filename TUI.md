# `next-tui` Terminal UI Reference

`next-tui` is a full-screen terminal UI for the same task manager the `next` CLI drives.
It is a thin [ratatui](https://ratatui.rs/) front-end that links the core library directly
(`TaskRepository`) — it never shells out to the `next` binary. Because it edits the
repository through the same core transactions and git commits as the CLI and the MCP
server, it is safe to run alongside both: every change is a normal task-file commit.

It ships as the `next-tui` binary behind the `tui` Cargo feature and shares the crate
version with `next`, `next-mcp`, and `next-forgejo` (there is no independent TUI
versioning).

---

## Building and running

```sh
# Build the binary
cargo build --release --features tui --bin next-tui

# Run it
next-tui
```

**Flags**

| Flag | Description |
|------|-------------|
| `--repo <path>` | Use this repository root instead of the configured/default repo or the upward directory search. |
| `--config <path>` | Use this config file instead of the `tui.toml` search. Reported as the active source. |
| `--version` / `-V` | Print `next-tui <version>` and exit (handled before the terminal is touched). |

Tracing logs go to stderr (invisible under the alternate screen); set `RUST_LOG` and
redirect stderr to a file to capture them.

---

## Configuration

The TUI reuses the same [`Config`] schema as the CLI but loads its **own** file,
`tui.toml`, with this precedence:

1. `$XDG_CONFIG_HOME/next/tui.toml` — the TUI's own config (source: `tui.toml`)
2. `$XDG_CONFIG_HOME/next/config.toml` — the CLI's config, used when `tui.toml` is absent (source: `config.toml`)
3. Built-in defaults (source: `defaults`)

`--config <path>` overrides the search entirely and is reported as the `tui.toml` source.
The active source is shown in the UI so you always know where the settings came from.

The relevant settings are the same as the CLI's: `repository`, `list_limit` (caps the list
view), and `forecast_horizon_days` (the initial forecast horizon).

---

## Views

`next-tui` has three top-level views, switched from Normal mode with `1`/`2`/`3` or cycled
with `Tab` (`List → Tree → Forecast → List`). It opens in the **Tree** view:

- **List** (`1`) — the scored, sorted flat task list, the same pipeline as `next list`
  (filters + scoring + the configured `list_limit`). Overdue / due-today rows are
  highlighted. Closed (done/cancelled) tasks are hidden by default; `.` toggles a
  closed-only view (like `next list --closed`), flagged as `[closed]` in the chrome; every
  closed task scores 0, so that view stays in completion order.
  A detail pane shows the selected task's fields, resolved
  parent/children/blockers, and its urgency-score breakdown (a bare `0.00` for a closed
  task).
- **Tree** (`2`) — the parent/child hierarchy (`tui-tree-widget`), shown by default on
  startup. Nodes expand/collapse; a tree-local toggle includes done/cancelled tasks. The
  detail pane and per-task actions operate on the highlighted node.
- **Forecast** (`3`) — the chronological forecast of upcoming due dates, including projected
  occurrences of both recurrence modes over the horizon (same engine as `next forecast`;
  completion-mode dates assume each instance is completed on its due date). It is
  a read-only, full-width list; the horizon is adjustable live.

The List and Tree views share the detail pane and the per-task action/edit keys. The active
view, repository root, config source, and active filters are shown in the chrome.

---

## Keymap

Keys are dispatched by mode, and within Normal mode by the active view. A key not listed
for the current mode/view is ignored.

### Normal mode — common to all views

| Key | Action |
|-----|--------|
| `q`, `Esc`, `Ctrl-c` | Quit |
| `Tab` | Cycle to the next view (List → Tree → Forecast) |
| `1` / `2` / `3` | Switch to List / Tree / Forecast |
| `r` | Reload (re-run load + score) |
| `/` | Open the filter bar (see [Filtering](#filtering)) |
| `S` (shift) | Open the state-management panel |
| `y` | Trigger a background sync (no-op if one is already running) |

### Filtering

`/` opens the filter bar, which takes the same expression language as
`next list` — see [CLI.md](CLI.md#filter-syntax) for the grammar. The list
re-filters **as you type**, so you can see what a query selects before
committing to it.

A half-typed expression is expected rather than an error: every prefix of
`due<+7d` is incomplete on the way in. While the buffer does not parse, the
last filter that did stays applied, the results on screen stay put, and a dim
hint beside the input says what is missing. `Enter` commits; `Esc` restores the
list as it was before you opened the bar.

Remember that a bare word searches the task text — `+tag` selects a tag.

### Per-task actions — List and Tree views

These operate on the selected list row or the highlighted tree node.

| Key | Action |
|-----|--------|
| `e` | Open the edit modal |
| `d` | Mark done (recurrence-aware) |
| `c` | Cancel |
| `s` | Toggle started / stopped |
| `o` | Open the task's URL in the system browser |
| `m` | Open the move (parent-picker) popup |
| `x`, `Delete` | Open the delete-confirmation popup |

### List view

| Key | Action |
|-----|--------|
| `j` / `Down` | Select next task |
| `k` / `Up` | Select previous task |
| `g` / `Home` | Select first task |
| `G` / `End` | Select last task |
| `.` | Toggle showing only closed (done/cancelled) tasks — mirrors `next list --closed`; respects the tag state and user filter |
| `A` (shift) | Toggle the `--all` filter |
| `F` (shift) | Toggle the `--future` filter |
| `U` (shift) | Toggle the `--all-users` filter |
| `Ctrl-d` / `PageDown` | Scroll the detail pane down |
| `Ctrl-u` / `PageUp` | Scroll the detail pane up |

### Tree view

| Key | Action |
|-----|--------|
| `j` / `Down` | Highlight next visible node |
| `k` / `Up` | Highlight previous visible node |
| `Left` | Collapse the highlighted node |
| `Right` | Expand the highlighted node |
| `Space` / `Enter` | Toggle expand/collapse |
| `.` | Toggle include-done/cancelled (tree-local) |
| `Ctrl-d` / `PageDown` | Scroll the detail pane down |
| `Ctrl-u` / `PageUp` | Scroll the detail pane up |

### Forecast view

A read-only list. Per-task actions and navigation do not apply.

| Key | Action |
|-----|--------|
| `+` / `=` | Widen the forecast horizon (by 30 days) |
| `-` / `_` | Narrow the forecast horizon (by 30 days) |
| `A` (shift) | Toggle the `--all` filter |
| `F` (shift) | Toggle the `--future` filter |
| `U` (shift) | Toggle the `--all-users` filter |

### Filter bar (`/`)

| Key | Action |
|-----|--------|
| `Enter` | Commit the buffer to the active filter and reload |
| `Esc` | Cancel editing and restore the previous tokens |
| *(any other key)* | Edit the filter buffer (text, backspace, arrows, …) |

The buffer accepts the same tokens as `next list` (`+tag`, `-tag`, `parent:`, `context:`,
`user:`). The `A` / `F` / `U` toggles cover `--all` / `--future` / `--all-users`.

### Edit modal (`e`)

Modal-level keys win; everything else is routed to the focused field. Fields include
title, due, start, priority, tags, assignee, url, score adjustment, long-term, the
recurrence fields (mode / rule / completion / snap), description, notes, and a data
key/value pair.

| Key | Action |
|-----|--------|
| `Ctrl-s` | Validate and save, then reload (modal stays open on error so you can retry) |
| `Esc` | Cancel and discard the form |
| `Tab` / `Shift-Tab` (`BackTab`) | Move focus to the next / previous field (always, even inside a textarea) |
| `Down` / `Up` | Move focus to the next / previous field — **except** inside the multi-line description/notes textareas, where they move the cursor |
| *(any other key)* | Edit the focused field |

### Delete confirmation (`x` / `Delete`)

| Key | Action |
|-----|--------|
| `y` / `Y` / `Enter` | Confirm and delete the task |
| `n` / `N` / `Esc` / `q` | Dismiss without deleting |

### Move / parent-picker (`m`)

A searchable list of valid parents (the task's own subtree is excluded to prevent cycles)
plus a "top-level / no parent" option.

| Key | Action |
|-----|--------|
| `Enter` | Reparent to the highlighted candidate |
| `Esc` | Dismiss without moving |
| `Down` / `Up` | Move the highlight |
| *(any other key)* | Edit the search buffer |

### State panel (`S`)

Two sections — Tags and Users — applied through the same `state_transaction` lock the
CLI's `next tag require|exclude|…` and `next user` commands use. The list reloads after
each toggle.

Tags are one list regardless of sigil, since every kind takes the same states. A row
shows the state that applies to it; one inherited from a parent tag is marked as such and
dimmed, because the row's key cycles its *own* entry, not the parent's.

| Key | Action |
|-----|--------|
| `Esc` / `q` / `S` | Close the panel |
| `Tab` / `Shift-Tab` (`BackTab`) | Focus the next / previous section |
| `j` / `Down` | Highlight the next row in the section |
| `k` / `Up` | Highlight the previous row in the section |
| `Space` / `Enter` / `a` | Cycle the highlighted tag: none → required → excluded → accepted → none (Tags section); toggle membership (Users section) |
| `x` | Run the tag cycle backwards, so a mis-press is one key away from undone (Tags section only) |
| `C` (shift) | Clear the focused section (every tag state / all users) |

---

## Feature parity and safety

`next-tui` covers the same day-to-day operations as the CLI: browse (list/tree/forecast),
filter, edit, complete (recurrence-aware), cancel, start/stop, move, delete, open URL,
manage machine-local state (tag state and the user filter), and sync. Every mutation goes
through the shared `core` services and `TaskRepository` transactions — the same code paths,
validation, git commits, and plugin export hooks as `next` and `next-mcp` — so running the
TUI alongside the CLI or the MCP server is safe. Sync runs on a background worker thread so
the UI stays responsive; its result surfaces in the footer.

For the full command-line surface and the underlying concepts (scoring, recurrence, filter
syntax), see [`CLI.md`](CLI.md). For internal design, see [`ARCHITECTURE.md`](ARCHITECTURE.md).
