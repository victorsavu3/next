# Task Manager — Goals

This file captures rough goals and ideas to be refined into structured documentation later.

## What I want to build

A CLI task manager built around GTD (Getting Things Done). The tool should make it fast
to capture tasks without friction and then process them into the right stage. Storage is a
local SQLite database so queries are fast and the data is portable.

Output should be pipe-friendly (clean JSON / plain text) so the tool integrates naturally
with Claude Code and other CLI tools rather than embedding AI directly.

## GTD stages

- **Inbox** — default landing zone; tasks land here on capture, no metadata required
- **Projects** — multi-step outcomes; tasks can belong to a project
- **Waiting-for** — blocked on someone else; tracks who and since when
- **Someday/Maybe** — low-commitment ideas to revisit later

## Key features

- **Capture** — add a task to inbox in one command with minimal typing
- **Process** — move tasks from inbox to the right stage / project
- **Due dates** — optional deadline on any task
- **Reminders** — surface overdue or due-today tasks prominently
- **Priority** — high / medium / low urgency on each task
- **Tags / contexts** — free-form labels (e.g. `@home`, `@work`, `+projectname`)
- **Pipe-friendly output** — `--json` flag or structured plain text for every list/show command so output can be piped to Claude Code or other tools

## Out of scope (for now)

- Built-in AI; AI is invoked externally via piped output
- Recurrence / repeating tasks
- Multi-machine sync (SQLite stays local)
- GUI or TUI

## Open questions

- **Reminders mechanism** — just show overdue tasks on every run, or also support OS desktop notifications / a background daemon?
- **Project hierarchy** — flat list of projects, or can projects be nested?
- **Waiting-for tracking** — store the person being waited on as free text, or a structured contact field?
- **Review workflow** — is there a weekly review command that walks through each stage interactively?
- **Database location** — `~/.local/share/task-manager/tasks.db` or configurable via env var?
