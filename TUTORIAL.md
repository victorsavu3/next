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
# $XDG_CONFIG_HOME/task-manager/config.toml
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
next edit <id> --tag @work --tag python     # replaces all tags
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

## Active context

Focus on one environment — only tasks tagged with that context (plus untagged tasks) are shown:

```sh
next context set @work      # show @work tasks + untagged tasks
next context clear          # show everything
next context                # show current context
```

Context-neutral tasks (no `@` tags) are always visible regardless of the active context.

---

## Resource availability

Hide tasks that require unavailable hardware:

```sh
next resource set #laptop off   # travelling without laptop
next list                       # #laptop tasks hidden
next resource set #laptop on    # back — tasks reappear
```

---

## Projects and subtasks

```sh
next add "Launch website" --slug launch
next add "Write copy" --parent launch
next add "Design logo" --parent launch
next tree                               # see the hierarchy
```

A parent task is hidden from the default list until all children are done or cancelled.

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
next list project:lang/rust            # tasks in lang/rust project
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

```sh
next add "Water plants" --recur-completion 3    # every 3 days after completion
next add "Weekly review" --recur-schedule "every Monday"
```

When you complete a recurrence task, a new instance is created automatically.

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
next --autosync add "Task"  # sync automatically after this command
```

To always sync after every mutation:

```toml
# config.toml
autosync = true
```

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
- **Tag factor** — each tag with explicit priority metadata adds an offset; applies to both the task's own tags and its parent's tags (`high` +1.0, `low` −0.5 by default)
- **Started bonus** — +4.0 when status is `started`; moves active tasks above idle peers
- **Manual adjustment** — `next edit <id> --adjust +2.0`

Tune any weight in the config:

```toml
[scoring]
priority_high  = 3.0
started_bonus  = 5.0
tag_high       = 2.0
age_per_day    = 0.02
```

---

## Quick reference

```
next add <title> [--priority low|medium|high] [--due DATE] [--tag TAG]...
next list [FILTERS]
next next [-n N]
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
next tag [describe | set-url | set-priority | data | show | clear-description | clear-url | clear-priority]
next context [set <@tag>... | clear]
next resource [set <#tag> on|off]
next user [set <name>... | clear | list]
next import forgejo <owner/repo> [--tag TAG]
```
