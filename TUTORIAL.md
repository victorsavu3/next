# next — Tutorial

`next` is a task manager that stores tasks as TOML files in a git repository and scores them by urgency so you always know what to work on first.

---

## Setup

```sh
mkdir ~/tasks && cd ~/tasks
next init
```

Point `next` at your repository from any directory:

```toml
# $XDG_CONFIG_HOME/next/config.toml
repository = "/home/you/tasks"
```

---

## Core workflow

```sh
next add "Buy groceries"                    # add a task
next list                                   # list open tasks by urgency
next next                                   # show the N most urgent tasks
next start <id>                             # mark as in-progress
next stop <id>                              # stop (back to open)
next done <id>                              # mark done
next cancel <id>                            # mark cancelled
```

`<id>` is the first 8 characters of the task UUID (shown in `next list`), or a slug you set with `--slug`.

```sh
next add "Fix login bug" --slug fix-login --priority high --due 2026-06-01
next done fix-login
```

---

## Editing tasks

```sh
next edit <id> --title "New title"
next edit <id> --priority high
next edit <id> --due 2026-12-31
next edit <id> --description "Details here"
next edit <id> --tag @work --tag python     # adds tags (use --remove-tag to remove)
```

Arbitrary key/value data on a task:

```sh
next data set <id> ticket JIRA-42
next data get <id> ticket
next data unset <id> ticket
```

---

## Tags

Three kinds of tag, all supporting `/`-separated hierarchies:

| Kind | Example | Meaning |
|------|---------|---------|
| Context | `@home`, `@work/frontend` | Where you are working |
| Resource | `#laptop`, `#office/printer` | What you need available |
| Freeform | `python`, `lang/rust` | Plain label |

```sh
next add "Write report" --tag @work --tag #laptop --tag python
```

Annotate tags:

```sh
next tag describe @work "Tasks done at the office"
next tag set-url #laptop "https://wiki/laptop"
next tag set-priority python high   # tasks tagged python default to high priority
next tag show @work                 # view all metadata for a tag
next tag                            # list all tags
```

---

## Tag state

Every tag — `@context`, `#resource` or plain — is required, excluded, or accepted. The
sigil says what a tag is for; it does not change how it filters.

Focus on one environment by **requiring** it. Only tasks carrying a required tag are
then listed:

```sh
next tag require @work      # only @work tasks
next tag clear-state        # show everything again
next tag                    # show the current state, then the tag list
```

Hide things by **excluding** them — the same command whatever the tag is:

```sh
next tag exclude '#laptop'  # travelling without the laptop
next list                   # #laptop tasks hidden
next tag clear-state '#laptop'
```

State is inherited by nested tags, and `accept` lets a child opt out of its parent's:

```sh
next tag exclude @home            # not doing home tasks…
next tag accept @home/kitchen     # …except in the kitchen
```

---

## Projects and subtasks

Any task with children is a project — no special type or tag is required. Use `--slug`
on the parent so you can reference it by name.

```sh
next add "Launch website" --slug launch
next add "Write copy" --parent launch
next add "Design logo" --parent launch
next tree                               # see the full hierarchy
next list parent:launch                # scored list of tasks under 'launch'
next show launch                       # full details for a single task
```

A parent task is hidden from the default list until all its direct children are done
or cancelled — focus stays on the leaf work.

Move or reparent a task:

```sh
next move <id> --parent <parent-id>
next move <id> --parent none           # remove from parent
```

---

## Filtering

All list commands accept filter tokens:

```sh
next list +python -bug                 # has 'python', doesn't have 'bug'
next list context:@work                # force context for this query
next list user:alice                   # show alice's tasks only
next list parent:launch                # all tasks under the 'launch' project
next list --all                        # disable all implicit filters
```

---

## Started state and time tracking

Mark a task as in-progress when you actively work on it:

```sh
next start <id>     # status → started; logs {event: "start", at: <timestamp>}
next stop <id>      # status → open;    logs {event: "stop",  at: <timestamp>}
```

Started tasks remain visible in `next list` and `next next` alongside open tasks. The
`data["time_log"]` array accumulates all start/stop events across multiple cycles:

```sh
next data get <id> time_log   # inspect the recorded entries
```

This data can be used for time-tracking analysis once tooling is built on top.

---

## Recurrence

Two modes are available. Completing a recurring task with `next done` automatically creates the next instance.

**Completion-based** — next instance is N days after you mark it done:

```sh
next add "Water plants" --recur-completion 7         # 1 week after completion
next add "Dentist check" --recur-completion 180       # ~6 months after completion
```

**Schedule-based** — next instance follows a fixed calendar pattern:

```sh
# Every weekday
next add "Daily standup" --slug standup \
  --recur-schedule "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"

# 1st of every month (task starts on the 1st, due on the 3rd)
next add "Monthly review" \
  --recur-schedule "FREQ=MONTHLY;BYMONTHDAY=1" \
  --start 2026-06-01 --due 2026-06-03

# Every 3 months on the 1st (quarterly)
next add "Quarterly planning" \
  --recur-schedule "FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1" \
  --start 2026-04-01
```

**Snap** — round the computed date to a convenient boundary:

```sh
# 1 week after completion, but always on a Saturday
next add "Weekly chore" --recur-completion 7 --recur-snap saturday

# Snap values: monday…sunday, next-workday, dom:N (day of month 1–28)
```

When a task has both `start` and `due` dates the offset is preserved — a task starting the 1st and due the 3rd will always have that 2-day window.

**Editing recurrence** — change or remove the rule after creation:

```sh
next edit standup --recur-snap monday    # change snap; rule and anchor unchanged
next edit standup --clear-recurrence     # remove recurrence entirely
```

---

## Forecast

```sh
next forecast              # tasks due in the next 90 days
next forecast --days 30    # shorter window
```

---

## Sync

```sh
next sync                  # git pull + git push
next sync --pull-only
next sync --push-only
next --autosync add "Task"  # pull before + push after this command
next --autopush add "Task"  # push after this command (no forced pull)
next --offline list         # skip all network I/O for one command
```

Before most commands `next` also pulls automatically when the local copy is more than
an hour old (**autopull**), so lists reflect other machines' changes. `--no-autopull`
(or `--offline`) skips it for one invocation.

To always push after every mutation:

```toml
# config.toml
[sync]
autopush = true
```

(or `next config set sync.autopush true`)

During sync, closed tasks older than 180 days are archived automatically (at most once
a day) into segment files under `archive/`. They stay visible via
`next list --archived` and still resolve by id or slug; editing one restores it.

If your SSH key is managed by a keychain or 1Password and the built-in git
bindings fail, use subprocess mode:

```toml
[sync]
git_subprocess = true
```

---

## Urgency scoring

The score shown in `next list` drives ordering. It combines:

- **Due-date factor** — dominates when a deadline is set; highest when overdue
- **Priority factor** — low / medium / high (+0 / +1 / +2)
- **Project factor** — parent task's priority offsets children (+0.5 / 0 / −0.5)
- **Age factor** — tasks grow slightly more urgent over time (capped at +2)
- **Tag factor** — each tag with explicit priority metadata adds an offset; applies to both the task's own tags and its parent's tags (`high` +1.0, `low` −1.0 by default)
- **Started bonus** — +4.0 when status is `started`; moves active tasks above idle peers
- **Manual adjustment** — `next edit <id> --adjust +2.0`

Tune any weight in `config/scoring.toml` **inside the task repository** (committed and
synced, so every machine and the MCP server score identically; omitted keys keep their
defaults):

```toml
# <repo>/config/scoring.toml
priority_high  = 3.0
started_bonus  = 5.0
tag_high       = 2.0
age_per_day    = 0.02
```

---

## Quick reference

```
next add <title> [--priority low|medium|high] [--due DATE] [--tag TAG]...
next list [FILTERS] [--archived] [--page N]
next next [N]
next show <id>
next start <id>
next stop <id>
next done <id>
next cancel <id>
next edit <id> [--title T] [--priority P] [--due D] [--tag TAG]...
next delete <id> --yes
next move <id> --parent <pid>
next tree [--all]
next forecast [--days N]
next sync [--pull-only | --push-only]
next tag [describe | set-url | set-priority | data | show | rename | clear-description | clear-url | clear-priority]
next tag [include <tag>... | exclude <tag>... | default <tag>... | clear-state [<tag>...]]
next user [set <name>... | clear | list]
next config [get <key> | set <key> <value>]
```
