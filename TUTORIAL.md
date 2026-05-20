# Tutorial

This tutorial walks through everyday use of `next`, a GTD-style task manager that stores tasks as TOML files in a git repository.

---

## Getting started

### Create a repository

`next` stores tasks inside a git repository. Create one (or use an existing project):

```
mkdir ~/tasks && cd ~/tasks
git init
```

You do not need a remote. All task data lives in `tasks/` as TOML files and is versioned automatically — every `add`, `done`, or `edit` operation creates a git commit.

Add `.next.db` to `.gitignore` to exclude the SQLite read-cache:

```
echo ".next.db" >> .gitignore
git add .gitignore && git commit -m "init: add .gitignore"
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

### GTD stages

Every task lives in one of four stages:

| Stage | Meaning |
|-------|---------|
| `inbox` | Captured, not yet processed (default) |
| `project` | Part of a committed multi-step project |
| `waiting` | Delegated or blocked on someone else |
| `someday` | Maybe later — excluded from the default list |

### Urgency score

`next list` sorts tasks by a computed urgency score. The score goes up with priority, proximity to the due date, task age, and parent priority. You can see raw scores with `next list --json`.

---

## Listing tasks

```
next list
```

Shows all open, unblocked tasks that are not in the future and not in `someday`, sorted by urgency.

Useful flags:

| Flag | Effect |
|------|--------|
| `--all` | Include done and cancelled tasks |
| `--future` | Include tasks whose start date is in the future |
| `--stage inbox` | Show only inbox tasks |

### Filter tokens

Pass filter tokens after any other arguments:

```
next list +@work          # only tasks tagged @work
next list -@home          # exclude tasks tagged @home
next list @work           # bare tag — same as +@work
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
| `--stage <stage>` | One of `inbox`, `project`, `waiting`, `someday` |
| `--wait-for <name>` | Sets stage to `waiting` and records who you're waiting on |
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

---

## Tags

Tags come in three flavours:

| Prefix | Kind | Example |
|--------|------|---------|
| `@` | Context | `@work`, `@home/office` |
| `#` | Resource | `#printer`, `#office/projector` |
| *(none)* | Freeform | `urgent`, `someday`, `python` |

Tags can be hierarchical using `/`. `@work/berlin` is a descendant of `@work`. Filtering by `@work` will include tasks tagged `@work/berlin`.

Freeform tags must start with a letter and may contain letters, digits, `-`, and `_`.

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
next resource set #printer false   # printer is broken — hide printer tasks
next resource set #printer true    # printer is fixed
next resource                      # list all resource states
```

---

## Projects and subtasks

Any task can act as a project. Create a parent first, then add subtasks:

```
next add "Launch blog" --stage project --slug launch-blog
next add "Write first post" --parent launch-blog
next add "Set up hosting"   --parent launch-blog --blocked-by "write-first-post"
```

A parent task is hidden from `next list` while any of its subtasks are still open.

---

## Getting the most urgent task

```
next next
```

Shows the top five tasks sorted by urgency. Useful as a quick "what do I do now?" command. Pass `--count N` to change the number shown.

---

## Waiting tasks

When you're waiting on someone:

```
next add "Review PR #42" --wait-for alice
```

Or convert an existing task:

```
next edit review-pr --wait-for alice
```

Waiting tasks are hidden from the default list. Show them with `--stage waiting`:

```
next list --stage waiting
```

---

## Someday / maybe

Capture ideas you're not committing to yet:

```
next add "Learn Japanese" --stage someday
```

Someday tasks are excluded from `next list` by default. Include them with `--stage someday` or `--all`.

---

## Blocking tasks

When task B cannot start until task A is done:

```
next add "Deploy to staging" --blocked-by "write-tests"
```

Blocked tasks are hidden from `next list` until all their blockers are completed.

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
next add "Weekly review" --recur-schedule "every Monday"
next add "Pay rent"      --recur-schedule "1st of every month"
```

Completion-based: repeats a fixed number of days after you mark it done.

```
next add "Water plants"       --recur-completion 3
next add "Clean coffee maker" --recur-completion 14
```

When you mark a recurring task done, a new instance is automatically created.

---

## Forecast

See which tasks are due in the next 30 days:

```
next forecast
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

This runs `git pull` (fast-forward), rebuilds the SQLite cache if HEAD changed, then `git push`. Conflicts are reported as file paths for manual resolution.

---

## Importing tasks

### Forgejo issues

```
next import forgejo https://forgejo.example.com/owner/repo
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

Asks for confirmation before permanently removing the task file. Pass `--force` to skip the prompt.

---

## Tips

**Slugs are your friend.** Give tasks you reference often a slug (`--slug water-plants`). Slugs are stable across renames and are far easier to type than UUID prefixes.

**Review weekly.** `next review` walks you through a GTD weekly review: process inbox, review projects, check waiting tasks, and clear stale someday items.

**Score adjustment.** Use `--adjust 10` to manually boost a task that should float to the top, or `--adjust -5` to push it down.

**Long-term tasks.** Use `--long-term` on tasks that are permanently in progress (like "exercise daily"). This disables age-based scoring so they do not accumulate urgency.
