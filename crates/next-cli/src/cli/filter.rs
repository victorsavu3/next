/// Shared filter arguments used across list-style commands.
///
/// Filter tokens are captured as trailing positional args and parsed into
/// typed fields by [`FilterArgs::parse`].
#[derive(Debug, Default)]
pub struct FilterArgs {
    pub required_tags: Vec<String>,
    pub excluded_tags: Vec<String>,
    pub project: Option<String>,
    pub context_override: Option<String>,
    pub future: bool,
    pub all: bool,
    pub stage: Option<String>,
    pub json: bool,
    /// Raw tokens that were not recognised as tag/project/context filters.
    pub raw_tokens: Vec<String>,
}

impl FilterArgs {
    /// Parse a flat list of positional token strings into a [`FilterArgs`].
    ///
    /// Recognised token forms
    /// ----------------------
    /// * `+tag`           → required_tags
    /// * `-tag`           → excluded_tags
    /// * `project:path`   → project
    /// * `context:@name`  → context_override
    ///
    /// Everything else is placed in `raw_tokens`.
    pub fn parse(tokens: Vec<String>) -> Self {
        let mut args = FilterArgs::default();
        for token in tokens {
            if let Some(tag) = token.strip_prefix('+') {
                args.required_tags.push(tag.to_owned());
            } else if let Some(tag) = token.strip_prefix('-') {
                args.excluded_tags.push(tag.to_owned());
            } else if let Some(path) = token.strip_prefix("project:") {
                args.project = Some(path.to_owned());
            } else if let Some(ctx) = token.strip_prefix("context:") {
                args.context_override = Some(ctx.to_owned());
            } else {
                args.raw_tokens.push(token);
            }
        }
        args
    }
}
