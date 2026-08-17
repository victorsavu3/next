# Working on `next`

Instructions for any agent (Claude Code, Codex, Cursor, …) making changes here.
Humans are welcome to follow them too.

## Build, test and lint through `just` — never raw `cargo`

```sh
just build          # compile, both feature sets
just test           # full suite, both feature sets
just test-one FOO   # one test or name filter, output shown
just lint           # clippy, warnings denied, both feature sets
just fmt            # format the tree
just fmt-check      # check formatting without writing
just check          # fmt-check + lint + test — what the hooks run
```

Two reasons, and the second is the one that bites:

**The recipes already do the filtering.** `just build` and `just test` print
diagnostics and a one-line summary, nothing else. A raw `cargo test` prints
several hundred lines of `... ok` to say the same thing, and the habit of
piping it through `tail`/`grep` to compensate produces a different ad-hoc
command every time — each of which needs its own approval, and several of
which quietly hide the failure they were meant to surface. (`cargo test |
grep -E '^test result'` reports nothing at all when the crate fails to
compile.) If you find yourself reaching for a pipeline, the recipe is missing
something: add it to the justfile rather than working around it.

**Both feature sets, every time.** The MCP surface is behind `--features mcp`.
`cargo test` alone runs the default set and `cargo test --all-features` runs
the other, and a test that compiles under one can fail under the other — that
is not hypothetical, it is why `just test` loops over both. `just lint` does
the same, including `--no-default-features`.

`just check` is the gate before you call a change done. The prek hooks
(`prek.toml`) run a subset of it on commit.

## Conventions

- **Tests belong with the code.** Unit tests go in a `#[cfg(test)] mod tests`
  in the same file; cross-command behaviour goes in `tests/`, using the harness
  in `tests/common/`. Do not verify by running the binary by hand — write the
  test.
- **Never run the `next` binary without `--repo`.** A bare invocation resolves
  to the real task repository in `~`, which autopushes. Tests use tempdirs.
- **Docs are part of the change.** `README.md` is the introduction,
  `CLI.md` the command reference, `ARCHITECTURE.md` the map,
  `REQUIREMENTS.md` the long-term plan. `tests/test_docs.rs` executes the
  filter examples in the docs, so an example that stops being true fails the
  suite.
- **Commit explicit paths.** `git add <path>`, never `git add -A` —
  `.claude/` and `CLAUDE.md` are local and gitignored, and sweeping them in
  is the mistake this rule exists to prevent. Keep commits small and on one
  topic.

## The filter language

`src/core/domain/filter_eval.rs` is the **reference semantics** for the filter
grammar. The SQL pushdown (`src/core/storage/sql_filter.rs`) and the FTS index
are optimisations that must agree with it, never the other way round, and the
differential tests in `tests/test_filter_pushdown.rs` use the evaluator as the
oracle. If you change what a predicate means, change it there first.

The pushdown's one rule: a translation may match a **superset** of what the
expression means, never a subset. A subset silently drops rows and nothing
fails. `SqlFilter::exact` records which it is, and only an exactly-translated
operand may be negated.
