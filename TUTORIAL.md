# Tutorial

This tutorial walks through everyday use of `next`, a task manager that stores tasks as
TOML files in a git repository and surfaces what to work on via automatic urgency scoring.

---

## Getting started

### Create a repository

`next` stores tasks inside a git repository. Use `next init` to set everything up in one step:

```
mkdir ~/tasks && cd ~/tasks
next init
```

This runs `git init`, creates the `tasks/` directory, and adds `.next.db` to `.gitignore`
automatically. All task data lives in `tasks/` as TOML files and is versioned
automatically — every `add`, `done`, or `edit` operation creates a git commit.

The repository layout looks like this after the first task is added:

```
~/tasks/
  .git/
  .gitignore          # contains ".next.db"
  tasks/
    buy-milk-3a7f1b2c.toml
  state.toml          # active contexts, resources, user filter
  .next.db            # SQLite read-cache — excluded from git
```

### Add your first task

```
next add "Buy milk"
```

`next` prints a short confirmation:

```
INFO  [add] added [3a7f1b2c] Buy milk
```

The eight-character code is the beginning of the task UUID and can be used to reference the task later.

---

## Core concepts

### Urgency score

`next list` sorts tasks by a computed urgency score. The score rises with priority,
proximity to the due date, task age, and parent priority. Tasks with the highest score
are shown first; `next next` shows only the top 10 as a quick "what do I do now?"

### Tasks, subtasks, and projects

Any task can have child tasks via `--parent`. A task becomes a **project** when you give
it the `project` tag (or create it with `next project add`). A parent task is hidden from
`next list` while any of its subtasks are still open.

---

## Listing tasks

```
next list
```

Shows all open, unblocked tasks whose start date is not in the future, sorted by urgency.

Useful flags:

| Flag | Effect |
|------|--------|
| `--all` | Include done and cancelled tasks; disable all implicit filtering |
| `--future` | Include tasks whose start date is in the future |

### Filter tokens

Pass filter tokens after any other arguments:

```
next list +@work          # only tasks tagged @work
next list -@home          # exclude tasks tagged @home
next list +urgent -@home  # combine filters
```

---

## Adding tasks with options

```
next add "Write weekly report" \
  --due "this Friday" \
  --priority high \
  --tag @work \
  --notes "Include Q2 numbers"
```

```
next add "Water plants" \
  --slug water-plants \
  --recur-completion 7
```

Common options:

| Option | Description |
|--------|-------------|
| `--due <date>` | ISO date (`2026-06-30`) or natural language (`in two weeks`, `tomorrow`) |
| `--start <date>` | Hide the task until this date |
| `--priority low\|medium\|high` | Default: `medium` |
| `--slug <name>` | Stable short name for referencing (`water-plants`) |
| `--tag <tag>` | Repeatable; use `@context`, `#resource`, or bare words |
| `--parent <id>` | UUID prefix or slug of the parent task |
| `--blocked-by <id>` | UUID prefix or slug of a blocking task |
| `--description <text>` | Multi-line context beyond the title |
| `--url <url>` | http/https URL for the task (ticket, doc, link) |
| `--assignee <user>` | Person responsible for the task |
| `--notes <text>` | Multi-line free text |
| `--recur-schedule "every Monday"` | Schedule-based recurrence |
| `--recur-completion 7` | Completion-based recurrence (days after done) |
| `--json` | Print the saved task as JSON |

---

## Marking tasks done

Reference a task by UUID prefix or slug:

```
next done water-plants
next done 3a7f1b2c
```

---

## Editing tasks

Use named flags to change specific fields:

```
next edit water-plants --due "next Sunday"
next edit water-plants --priority high
next edit 3a7f1b2c --title "Buy oat milk"
next edit 3a7f1b2c --clear-due
```

Add or remove tags with trailing `+tag` / `-tag` tokens or with `--tag` / `--remove-tag`:

```
next edit water-plants +@home -urgent
next edit water-plants --tag @garden --remove-tag urgent
```

Set or clear the description and URL:

```
next edit my-ticket --url "https://example.com/ticket-42"
next edit my-ticket --description "See comment from Alice in the ticket"
next edit my-ticket --clear-url
```

---

## Tags

Tags come in three flavours:

| Prefix | Kind | Example |
|--------|------|---------|
| `@` | Context | `@work`, `@home/office` |
| `#` | Resource | `#printer`, `#office/projector` |
| *(none)* | Freeform | `urgent`, `project`, `python` |

Tags can be hierarchical using `/`. `@work/berlin` is a descendant of `@work`. Filtering
by `@work` will include tasks tagged `@work/berlin`.

---

## Contexts

Contexts filter your view to tasks relevant to where you are right now.

```
next context set @home         # only show tasks tagged @home (or descendants)
next context                   # show active contexts
next context clear             # show everything again
```

Active contexts are stored in `state.toml` and persist across sessions.

---

## Resources

Resources represent equipment or conditions. Mark a resource unavailable to hide tasks that require it:

```
next resource set #printer off   # printer is broken — hide printer tasks
next resource set #printer on    # printer is fixed
next resource                    # list all resource states
```

---

## Projects and subtasks

Create a project task and add subtasks to it:

```
next project add "Launch blog" --slug launch-blog
next add "Write first post" --parent launch-blog
next add "Set up hosting" --parent launch-blog

next project list              # tree view of all projects
next project show launch-blog  # show project and all subtasks
```

A parent task is hidden from `next list` while any of its subtasks are still open.
Long-running projects should use `--long-term` to avoid accumulating age-based urgency.

---

## Blocking tasks

When task B cannot start until task A is done:

```
next add "Deploy to staging" --blocked-by "write-tests"
```

Blocked tasks are hidden from `next list` until all their blockers are completed.

---

## Opening URLs

When a task has a URL (e.g. a ticket or doc), open it directly:

```
next open my-ticket
next open a1b2c3d4
```

---

## Task data

Store arbitrary key-value metadata on a task:

```
next data set a1b2 source "github"
next data set a1b2 score 42
next data set a1b2 urgent true
next data get a1b2 source
next data unset a1b2 source
```

This is useful for AI-provided metadata or tool integrations.

---

## Showing full task details

```
next show water-plants
next show 3a7f1b2c
```

---

## Recurring tasks

Schedule-based: repeats on a calendar pattern regardless of when you complete it.

```
next add "Pay rent"     --recur-schedule "1st of every month"
next add "Team standup" --recur-schedule "every weekday"
```

Completion-based: repeats a fixed number of days after you mark it done.

```
next add "Water plants"       --recur-completion 3
next add "Clean coffee maker" --recur-completion 14
```

When you mark a recurring task done, a new instance is automatically created.

---

## Forecast

See which tasks are due in the coming months:

```
next forecast
next forecast --days 180
```

---

## User filters (team use)

When multiple people share a repository, `next user` scopes the list to one or more assignees:

```
next user set alice
next list             # shows alice's tasks + unassigned tasks
next user clear       # back to everyone
```

You can also filter by user inline:

```
next list user:alice
```

---

## Syncing with a remote

If you have a git remote:

```
next sync
```

This runs `git pull` (fast-forward), rebuilds the SQLite cache if HEAD changed, then
`git push`. Conflicts are reported as file paths for manual resolution.

---

## Importing tasks

### Forgejo issues

```
next import forgejo owner/repo
```

Creates a task for each open issue. Re-running updates existing tasks; it does not create duplicates.

### iCalendar

```
next import ical ~/calendar.ics
```

Imports `VTODO` entries. Re-running is safe — tasks are matched by iCalendar UID.

---

## Exporting tasks

```
next export ical > tasks.ics
```

---

## Deleting tasks

```
next delete 3a7f1b2c
```

Asks for confirmation before permanently removing the task file.

---

## Tips

**Slugs are your friend.** Give tasks you reference often a slug (`--slug water-plants`).
Slugs are stable across renames and are far easier to type than UUID prefixes.

**Score adjustment.** Use `--adjust 10` to manually boost a task that should float to
the top, or `--adjust -5` to push it down.

**Long-term tasks.** Use `--long-term` on tasks that are permanently in progress (like
"exercise daily"). This disables age-based scoring so they do not accumulate urgency.

**Description vs notes.** Use `--description` for a brief context summary that appears
in the task list output; use `--notes` for longer free-form reference material.
