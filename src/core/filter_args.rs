use crate::core::domain::filter::FilterSet;
use crate::core::domain::filter_expr::{self, Expr, Overrides};

/// Shared filter arguments used across list-style commands.
///
/// The query itself is a single [`Expr`]: every surface joins whatever it
/// captured — CLI trailing argv, the TUI filter bar, the MCP token array — into
/// one string, parses it here, and evaluates the same tree. That is what keeps
/// `next list`, the TUI and an agent from drifting into three dialects.
///
/// The flags beside it are not part of the grammar: they widen or narrow which
/// tasks are *candidates* (`--all`, `--closed`, `--future`, `--all-users`)
/// rather than describing a task, and they stay ordinary flags.
#[derive(Debug, Default)]
pub struct FilterArgs {
    /// The parsed query. `And(vec![])` — the default — matches everything.
    pub expr: Expr,
    /// The view terms lifted out of the query: `parent:`, `context:`, `user:`.
    pub overrides: Overrides,
    /// When true, bypass the user filter entirely (show tasks for all users).
    pub all_users: bool,
    pub future: bool,
    pub all: bool,
    pub closed: bool,
    pub json: bool,
}

/// Reject trailing tokens that begin with `--`.
///
/// Commands that capture trailing tokens use `trailing_var_arg` +
/// `allow_hyphen_values` so that `-tag` removal/exclusion tokens parse; a side
/// effect is that clap hands us unknown `--flags` (usually typos of real
/// flags, e.g. `--clear-du` for `--clear-due`) as ordinary tokens. Tags and
/// filter tokens never legitimately start with `--`, so fail loudly instead
/// of silently misinterpreting the token.
pub fn reject_flag_like_tokens(tokens: &[String], help_cmd: &str) -> anyhow::Result<()> {
    for token in tokens {
        if token.starts_with("--") {
            anyhow::bail!(
                "unrecognised flag '{token}' — run `{help_cmd}` to see the available flags"
            );
        }
    }
    Ok(())
}

impl FilterArgs {
    /// Parses argv-style tokens into a filter.
    ///
    /// The tokens are joined with a space and parsed as one expression, so
    /// `next list +@work -bug` and `next list '+@work -bug'` are the same
    /// query and only phrases, parentheses and `#resource` tags need shell
    /// quoting.
    ///
    /// Note what a *bare* token now means: `bug` searches the task text. The
    /// tag it used to select is `+bug`.
    pub fn parse(tokens: Vec<String>) -> anyhow::Result<Self> {
        Self::parse_query(&tokens.join(" "))
    }

    /// Parses an already-joined query string.
    pub fn parse_query(query: &str) -> anyhow::Result<Self> {
        let parsed = filter_expr::parse(query).map_err(|e| anyhow::anyhow!(e))?;
        let (expr, overrides) =
            filter_expr::lift_overrides(parsed).map_err(|e| anyhow::anyhow!(e))?;
        Ok(FilterArgs {
            expr,
            overrides,
            ..Default::default()
        })
    }

    /// Converts to a domain [`FilterSet`].
    pub fn to_filter_set(&self) -> anyhow::Result<FilterSet> {
        let user_override = if self.all_users {
            Some(vec![]) // empty = bypass user filter
        } else {
            self.overrides.users.clone()
        };
        Ok(FilterSet {
            expr: self.expr.clone(),
            required_override: self.overrides.required_override.clone(),
            user_override,
            include_future: self.future,
            disable_implicit: self.all,
            closed_only: self.closed,
            include_blocked_parents: false,
            parent_slug: self.overrides.parent_slug.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(tokens: &[&str]) -> FilterArgs {
        FilterArgs::parse(tokens.iter().map(|s| s.to_string()).collect())
            .unwrap_or_else(|e| panic!("{tokens:?}: {e}"))
    }

    fn err(tokens: &[&str]) -> String {
        FilterArgs::parse(tokens.iter().map(|s| s.to_string()).collect())
            .expect_err("expected a parse error")
            .to_string()
    }

    #[test]
    fn no_tokens_is_the_identity() {
        assert_eq!(args(&[]).expr, Expr::And(Vec::new()));
        assert_eq!(args(&[]).overrides, Overrides::default());
    }

    #[test]
    fn tokens_are_joined_into_one_expression() {
        // Whether the shell split the query is not the query's business.
        assert_eq!(args(&["+@work", "-bug"]).expr, args(&["+@work -bug"]).expr);
    }

    #[test]
    fn view_terms_are_lifted_out_of_the_query() {
        let parsed = args(&["parent:infra", "context:@work", "user:alice", "+bug"]);
        assert_eq!(parsed.overrides.parent_slug.as_deref(), Some("infra"));
        assert_eq!(
            parsed.overrides.required_override.as_deref(),
            Some(["@work".to_owned()].as_slice())
        );
        assert_eq!(
            parsed.overrides.users.as_deref(),
            Some(["alice".to_owned()].as_slice())
        );
        // What is left is the per-task predicate alone.
        assert_eq!(parsed.expr.to_string(), "+bug");
    }

    #[test]
    fn repeated_user_terms_still_widen_the_view() {
        // `user:alice user:bob` shows both people's tasks, as it always has;
        // `user:alice,bob` is the same thing written as a set.
        for tokens in [
            ["user:alice", "user:bob"].as_slice(),
            ["user:alice,bob"].as_slice(),
        ] {
            let parsed = args(tokens);
            assert_eq!(
                parsed.overrides.users.as_deref(),
                Some(["alice".to_owned(), "bob".to_owned()].as_slice()),
                "{tokens:?}"
            );
        }
    }

    #[test]
    fn a_view_term_lifts_from_anywhere_in_the_conjunction() {
        // `and` is associative, so parentheses around part of a conjunction
        // must not change what the query means. Before this was flattened,
        // the nested `and` was refused for being "inside an or or a not".
        for query in [
            "+bug parent:infra +x",
            "+bug and (parent:infra and +x)",
            "(+bug and parent:infra) and +x",
            "((parent:infra))",
        ] {
            let parsed = args(&[query]);
            assert_eq!(
                parsed.overrides.parent_slug.as_deref(),
                Some("infra"),
                "{query}"
            );
        }
    }

    #[test]
    fn a_view_term_inside_a_boolean_is_refused() {
        for tokens in [
            ["+bug", "or", "parent:infra"].as_slice(),
            ["not", "user:alice"].as_slice(),
            ["(context:@work or +bug)"].as_slice(),
        ] {
            let message = err(tokens);
            assert!(
                message.contains("cannot appear inside"),
                "{tokens:?}: {message}"
            );
        }
    }

    #[test]
    fn parent_may_only_scope_once() {
        assert!(err(&["parent:a", "parent:b"]).contains("only be given once"));
        // A set is a different mistake from a repeat, and says so.
        let set = err(&["parent:a,b"]);
        assert!(set.contains("one slug rather than a set"), "{set}");
        assert!(!set.contains("only be given once"), "{set}");
    }

    #[test]
    fn score_is_refused_with_a_reason() {
        let message = err(&["score>5"]);
        assert!(message.contains("computed after filtering"), "{message}");
    }

    #[test]
    fn a_malformed_query_is_an_error_not_a_silent_match() {
        assert!(err(&["due<"]).contains("cannot parse filter expression"));
        assert!(err(&["+not-a-tag/"]).contains("cannot parse filter expression"));
    }

    #[test]
    fn all_users_beats_a_user_term() {
        let mut parsed = args(&["user:alice"]);
        parsed.all_users = true;
        assert_eq!(parsed.to_filter_set().unwrap().user_override, Some(vec![]));
    }

    #[test]
    fn flags_become_filter_set_fields() {
        let mut parsed = args(&["+bug"]);
        parsed.future = true;
        parsed.all = true;
        parsed.closed = true;
        let set = parsed.to_filter_set().unwrap();
        assert!(set.include_future && set.disable_implicit && set.closed_only);
        assert_eq!(set.expr, parsed.expr);
    }

    #[test]
    fn flag_like_tokens_are_rejected() {
        let tokens = vec!["--clear-du".to_string()];
        assert!(reject_flag_like_tokens(&tokens, "next list --help").is_err());
        assert!(reject_flag_like_tokens(&["-bug".to_string()], "next list --help").is_ok());
    }
}
