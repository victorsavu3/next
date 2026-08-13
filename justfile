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

# Run one test (or a name filter) under all features, with output shown.
test-one filter:
    cargo test --workspace --all-features -- --nocapture {{ filter }}

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
