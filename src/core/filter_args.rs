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

/// The flags a command declares, as told apart from filter tokens.
///
/// Built from the clap `Command` by the CLI (see
/// `crate::cli::commands::reject_misplaced_flags`) rather than written out by
/// hand: a hand-maintained list drifts the moment someone adds a flag, and a
/// drifted list is exactly the bug this type exists to prevent.
#[derive(Debug, Default, Clone)]
pub struct KnownFlags {
    long: std::collections::BTreeSet<String>,
    short: std::collections::BTreeSet<char>,
}

impl KnownFlags {
    /// Collects the long names (without `--`) and short names (without `-`).
    pub fn new(
        long: impl IntoIterator<Item = String>,
        short: impl IntoIterator<Item = char>,
    ) -> Self {
        KnownFlags {
            long: long.into_iter().collect(),
            short: short.into_iter().collect(),
        }
    }

    /// Whether `token` — a trailing argv token — names one of these flags.
    ///
    /// `--fields` and `--fields=id` are the same flag. A short flag matches
    /// only the exact two-character form: `-n` is `--limit`, while `-bug`,
    /// `-@work` and `-#printer` are tag exclusions and must stay that way.
    fn matches(&self, token: &str) -> bool {
        if let Some(long) = token.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            return self.long.contains(name);
        }
        let Some(rest) = token.strip_prefix('-') else {
            return false;
        };
        let mut chars = rest.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => self.short.contains(&c),
            _ => false,
        }
    }
}

/// Reject trailing tokens that are really flags.
///
/// Commands that capture trailing tokens use `trailing_var_arg` +
/// `allow_hyphen_values` so that `-tag` exclusion tokens parse; the side effect
/// is that everything after the first filter token — including the command's
/// own flags — arrives here as ordinary tokens. Two things can go wrong, and
/// they need different answers:
///
/// * a flag the command really has (`next list +@work --fields id`, or `-n 5`,
///   which would otherwise read as "not the tag `n`" and quietly match
///   nothing) — it is in the right command but the wrong position, so say so;
/// * an unknown `--flag`, usually a typo of a real one (`--clear-du` for
///   `--clear-due`) — tags never start with `--`, so refuse it outright.
///
/// A tag exclusion (`-bug`, `-@work`, `-#printer`) is neither and passes
/// through wherever it appears.
pub fn reject_flag_like_tokens(
    tokens: &[String],
    flags: &KnownFlags,
    command: &str,
) -> anyhow::Result<()> {
    for token in tokens {
        if flags.matches(token) {
            let name = token.split('=').next().unwrap_or(token);
            anyhow::bail!(
                "`{name}` is a flag of `{command}`, not a filter token — flags must come \
                 before the filter expression, as in `{command} {name} … <filter>`"
            );
        }
        if token.starts_with("--") {
            anyhow::bail!(
                "unrecognised flag '{token}' — run `{command} --help` to see the available flags"
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
        assert!(err(&["parent:a,b"]).contains("only be given once"));
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

    /// The flags `next list` declares, as clap would report them.
    fn list_flags() -> KnownFlags {
        KnownFlags::new(
            ["fields", "count", "json", "limit", "page-size"].map(str::to_owned),
            ['n', 'h'],
        )
    }

    fn reject(token: &str) -> Option<String> {
        reject_flag_like_tokens(&[token.to_string()], &list_flags(), "next list")
            .err()
            .map(|e| e.to_string())
    }

    #[test]
    fn an_unknown_flag_like_token_is_rejected() {
        // A typo of a real flag: `--` is never the start of a tag, so there is
        // nothing else this could have meant.
        let message = reject("--clear-du").expect("a typoed flag is refused");
        assert!(message.contains("unrecognised flag"), "{message}");
        assert!(message.contains("next list --help"), "{message}");
    }

    #[test]
    fn a_real_flag_in_trailing_position_is_told_where_it_belongs() {
        // These are all listed by `--help`, so "unrecognised" would contradict
        // the help text the message sends the reader to.
        for token in ["--fields", "--fields=id,title", "--count", "--json", "-n"] {
            let message = reject(token).unwrap_or_else(|| panic!("{token} must be refused"));
            assert!(
                message.contains("before the filter expression"),
                "{token}: {message}"
            );
            assert!(!message.contains("unrecognised"), "{token}: {message}");
            // The message names the flag, not the flag plus its value.
            assert!(message.contains("`--fields`") || !token.starts_with("--fields"));
        }
    }

    #[test]
    fn a_tag_exclusion_is_not_a_flag() {
        // The whole reason `allow_hyphen_values` is on: these must survive,
        // including next to a short flag name (`-n` is a flag, `-note` is not).
        for token in ["-bug", "-@work", "-#printer", "-note", "-n5"] {
            assert!(reject(token).is_none(), "{token} is a tag exclusion");
        }
    }
}
