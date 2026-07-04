use crate::core::domain::filter::FilterSet;
use crate::core::domain::tag;

/// Shared filter arguments used across list-style commands.
///
/// Filter tokens are captured as trailing positional args and parsed into
/// typed fields by [`FilterArgs::parse`].
#[derive(Debug, Default)]
pub struct FilterArgs {
    pub required_tags: Vec<String>,
    pub excluded_tags: Vec<String>,
    pub parent: Option<String>,
    pub context_override: Option<String>,
    /// `user:alice user:bob` → restrict to tasks assigned to alice or bob.
    pub user_override: Option<Vec<String>>,
    /// When true, bypass the user filter entirely (show tasks for all users).
    pub all_users: bool,
    pub future: bool,
    pub all: bool,
    pub json: bool,
}

impl FilterArgs {
    /// Parse a flat list of positional token strings into a [`FilterArgs`].
    ///
    /// Recognised token forms:
    /// - `+tag` or bare `tag` → required_tags
    /// - `-tag`               → excluded_tags
    /// - `parent:slug`        → parent
    /// - `context:@name`      → context_override
    /// - `user:name`          → user_override
    pub fn parse(tokens: Vec<String>) -> Self {
        let mut args = FilterArgs::default();
        for token in tokens {
            if let Some(tag) = token.strip_prefix('+') {
                args.required_tags.push(tag.to_owned());
            } else if let Some(tag) = token.strip_prefix('-') {
                args.excluded_tags.push(tag.to_owned());
            } else if let Some(slug) = token.strip_prefix("parent:") {
                args.parent = Some(slug.to_owned());
            } else if let Some(ctx) = token.strip_prefix("context:") {
                args.context_override = Some(ctx.to_owned());
            } else if let Some(user) = token.strip_prefix("user:") {
                args.user_override
                    .get_or_insert_with(Vec::new)
                    .push(user.to_owned());
            } else {
                // Bare token: treat as required tag (same as +tag).
                args.required_tags.push(token);
            }
        }
        args
    }

    /// Converts to a domain [`FilterSet`].
    ///
    /// Tag tokens are validated, so a malformed `+tag` / `-tag` surfaces as an
    /// `Err` rather than silently matching nothing.
    pub fn to_filter_set(&self) -> anyhow::Result<FilterSet> {
        for t in self.required_tags.iter().chain(self.excluded_tags.iter()) {
            tag::validate_tag(t).map_err(|e| anyhow::anyhow!(e))?;
        }
        let user_override = if self.all_users {
            Some(vec![]) // empty = bypass user filter
        } else {
            self.user_override.clone()
        };
        Ok(FilterSet {
            required_tags: self.required_tags.clone(),
            excluded_tags: self.excluded_tags.clone(),
            context_override: self
                .context_override
                .as_ref()
                .map(|c| vec![c.clone()]),
            user_override,
            include_future: self.future,
            disable_implicit: self.all,
            include_blocked_parents: false,
            parent_slug: self.parent.clone(),
            closed_only: false,
        })
    }
}
