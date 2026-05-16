/// Classifies a tag string by its prefix convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    /// `@name` — working-environment context (e.g. `@home`, `@work`).
    Context,
    /// `$name` — physical or situational resource (e.g. `$printer`, `$vacation`).
    Resource,
    /// No prefix — freeform label (e.g. `python`, `reading`).
    Freeform,
}

/// Returns the kind of a tag based on its prefix.
pub fn classify(tag: &str) -> TagKind {
    if tag.starts_with('@') {
        TagKind::Context
    } else if tag.starts_with('$') {
        TagKind::Resource
    } else {
        TagKind::Freeform
    }
}

/// Returns `true` if `tag` is a context tag (`@` prefix).
pub fn is_context(tag: &str) -> bool {
    matches!(classify(tag), TagKind::Context)
}

/// Returns `true` if `tag` is a resource tag (`$` prefix).
pub fn is_resource(tag: &str) -> bool {
    matches!(classify(tag), TagKind::Resource)
}

/// Strips the leading `@` or `$` prefix and returns the bare name.
pub fn bare_name(tag: &str) -> &str {
    tag.trim_start_matches(['@', '$'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_tag() {
        assert_eq!(classify("@home"), TagKind::Context);
        assert_eq!(classify("@work"), TagKind::Context);
    }

    #[test]
    fn resource_tag() {
        assert_eq!(classify("$printer"), TagKind::Resource);
        assert_eq!(classify("$vacation"), TagKind::Resource);
    }

    #[test]
    fn freeform_tag() {
        assert_eq!(classify("python"), TagKind::Freeform);
        assert_eq!(classify("reading"), TagKind::Freeform);
        assert_eq!(classify(""), TagKind::Freeform);
    }

    #[test]
    fn bare_name_strips_prefix() {
        assert_eq!(bare_name("@home"), "home");
        assert_eq!(bare_name("$printer"), "printer");
        assert_eq!(bare_name("python"), "python");
    }
}
