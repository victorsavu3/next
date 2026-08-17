# Development tasks for `next`.
#
# The recipes mirror the prek pre-commit hooks (see prek.toml) so that what you
# run locally is what the commit will run — with one deliberate addition:
# `test` covers the DEFAULT feature set as well as `--all-features`. The hooks
# only run the latter, which is how a test file calling the feature-gated MCP
# API once broke `cargo test` for a week without anything going red.

# List the available recipes.
default:
    @just --list

# Compile under both feature sets, printing only diagnostics.
#
# `--all-targets` so a broken test or bench is a build failure here rather than
# a surprise at `just test`, which is the slower way to find out.
build:
    #!/usr/bin/env bash
    set -uo pipefail
    status=0
    for features in "default" "all-features"; do
        flag=""
        [ "$features" = "all-features" ] && flag="--all-features"
        out=$(cargo build --workspace --all-targets $flag 2>&1)
        if [ $? -ne 0 ]; then
            echo "── $features: FAILED ──"
            grep -E "^(error|warning)(\[|:)" -A 8 <<<"$out" | head -80
            status=1
        else
            # Warnings do not fail the build, but they fail `just lint`, so
            # surfacing them here saves a round trip.
            warnings=$(grep -cE "^warning(\[|:)" <<<"$out")
            if [ "$warnings" -gt 0 ]; then
                echo "$features: ok, $warnings warning(s)"
                grep -E "^warning(\[|:)" -A 6 <<<"$out" | head -40
            else
                echo "$features: ok"
            fi
        fi
    done
    exit $status

# Run the suite under both feature sets, printing only what failed.
test:
    #!/usr/bin/env bash
    set -uo pipefail
    status=0
    for features in "default" "all-features"; do
        flag=""
        [ "$features" = "all-features" ] && flag="--all-features"
        out=$(cargo test --workspace $flag 2>&1)
        # Distinguish "did not compile" from "compiled and failed" by whether
        # any test actually ran: cargo prints `error: test failed` for a plain
        # assertion failure too, so matching on `^error` alone mislabels it.
        if ! grep -q "^test result" <<<"$out"; then
            echo "── $features: DID NOT BUILD ──"
            grep -E "^error(\[|:)" -A 6 <<<"$out" | head -60
            status=1
            continue
        fi
        if grep -q "^test result: FAILED" <<<"$out"; then
            echo "── $features: FAILURES ──"
            # The `---- <name> stdout ----` blocks name each failing test and
            # carry its panic message, so they are the whole story; the
            # trailing `failures:` name list would just repeat them.
            grep -E "^---- " -A 8 <<<"$out" | head -150
            status=1
        fi
        echo "$features: $(grep -E '^test result' <<<"$out" \
            | awk '{p+=$4; f+=$6} END {print p" passed, "f" failed"}')"
    done
    exit $status

# Run one test (or a name filter) under all features, with its output shown.
#
# Only the targets that actually ran something are reported: a name filter
# matches nothing in most of the two dozen test binaries, and printing
# `running 0 tests` for each of them buries the one result being asked for.
test-one filter:
    #!/usr/bin/env bash
    set -uo pipefail
    out=$(cargo test --workspace --all-features -- --nocapture {{ filter }} 2>&1)
    status=$?
    if ! grep -q "^test result" <<<"$out"; then
        echo "── DID NOT BUILD ──"
        grep -E "^error(\[|:)" -A 6 <<<"$out" | head -60
        exit 1
    fi
    # Keep each block from `Running <target>` through its result line, but only
    # where a test ran; plus panics and the `---- <name> stdout ----` bodies.
    awk '
        /^ *Running |^ *Doc-tests /   { target = $0; next }
        /^running 0 tests/            { next }
        /^running [0-9]+ test/        { if (target) { print target; target = "" } }
                                      { if (!/^$/) print }
    ' <<<"$out" | grep -vE "^test result: ok\. 0 passed"
    echo "── $(grep -E '^test result' <<<"$out" \
        | awk '{p+=$4; f+=$6} END {print p" passed, "f" failed"}') ──"
    exit $status

# Install every binary into ~/.cargo/bin from the working tree.
#
# All four (`next`, `next-mcp`, `next-forgejo`, `next-tui`) are gated behind
# required-features, so anything short of `--all-features` silently installs a
# subset — which is how a stale `next-mcp` ends up serving an old tool schema
# long after the CLI was rebuilt.
install:
    #!/usr/bin/env bash
    set -uo pipefail
    out=$(cargo install --path . --all-features --locked --force 2>&1)
    if [ $? -ne 0 ]; then
        grep -E "^(error|warning)(\[|:)" -A 6 <<<"$out" | head -40
        exit 1
    fi
    grep -E "^ *(Installed|Replaced|Replacing)" <<<"$out"

# Everything the pre-commit hook checks, plus the default-feature test run.
check: fmt-check lint test

# Format the tree.
fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

# Clippy under both feature sets, warnings denied — as the hooks run it.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo clippy --workspace --no-default-features -- -D warnings
