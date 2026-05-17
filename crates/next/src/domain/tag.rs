/// Classifies a tag string by its prefix convention.
///
/// All three kinds support hierarchical nesting via `/` separators.
/// A filter on a parent segment matches any descendant: `@work` matches
/// `@work/frontend`, `$office` matches `$office/printer`, `#lang` matches
/// `#lang/rust`. Use [`tag_matches`] to test this relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    /// `@name` or `@parent/child` — working-environment context (e.g. `@home`, `@work/frontend`).
    Context,
    /// `$name` or `$parent/child` — physical or situational resource (e.g. `$printer`, `$office/desk`).
    /// If a parent resource is unavailable, all its descendants are implicitly unavailable too.
    Resource,
    /// `#name` or `#parent/child` — freeform hierarchical label (e.g. `#python`, `#lang/rust`).
    Freeform,
}

/// Returns the kind of a tag based on its prefix.
pub fn classify(tag: &str) -> TagKind {
    if tag.starts_with('@') {
        TagKind::Context
    } else if tag.starts_with('$') {
        TagKind::Resource
    } else {
        // Both `#name` and bare names (legacy) are Freeform.
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

/// Strips the leading `@`, `$`, or `#` prefix and returns the bare name.
///
/// For hierarchical tags the full path is returned: `bare_name("#work/backend") == "work/backend"`.
pub fn bare_name(tag: &str) -> &str {
    tag.trim_start_matches(['@', '$', '#'])
}

/// Returns `true` when `filter` matches `tag`.
///
/// The same rules apply to all prefix kinds (`@`, `$`, `#`):
/// - Exact match: `filter == tag`.
/// - Ancestor match: `tag` starts with `filter` followed immediately by `/`.
///
/// Examples:
/// ```
/// use next::domain::tag::tag_matches;
/// assert!(tag_matches("#abc",      "#abc"));         // exact
/// assert!(tag_matches("#abc",      "#abc/cde"));     // ancestor
/// assert!(tag_matches("@work",     "@work/frontend")); // context hierarchy
/// assert!(tag_matches("$office",   "$office/desk"));  // resource hierarchy
/// assert!(!tag_matches("#abc/cd",  "#abc/cde"));    // partial segment
/// assert!(!tag_matches("#ab",      "#abc/cde"));    // different segment
/// assert!(!tag_matches("#abc",     "#abcdef"));     // no slash boundary
/// ```
pub fn tag_matches(filter: &str, tag: &str) -> bool {
    if tag == filter {
        return true;
    }
    // Check that `filter` is a complete segment prefix of `tag`.
    match tag.strip_prefix(filter) {
        Some(rest) => rest.starts_with('/'),
        None => false,
    }
}

/// Returns all ancestor tag strings for `tag`, from outermost to `tag` itself.
///
/// For `#a/b/c` returns `["#a", "#a/b", "#a/b/c"]`. The prefix character is
/// preserved. For a tag with no `/`, returns only the tag itself.
pub fn ancestors(tag: &str) -> Vec<&str> {
    let (prefix, rest) = if let Some(s) = tag.strip_prefix(['@', '$', '#']) {
        (&tag[..1], s)
    } else {
        ("", tag)
    };

    let mut result = Vec::new();
    let pos = prefix.len();
    for (i, c) in rest.char_indices() {
        if c == '/' {
            result.push(&tag[..pos + i]);
        }
    }
    result.push(tag); // include the tag itself
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_tag() {
        assert_eq!(classify("@home"), TagKind::Context);
        assert_eq!(classify("@work"), TagKind::Context);
        assert_eq!(classify("@work/frontend"), TagKind::Context);
    }

    #[test]
    fn resource_tag() {
        assert_eq!(classify("$printer"), TagKind::Resource);
        assert_eq!(classify("$vacation"), TagKind::Resource);
    }

    #[test]
    fn freeform_tag() {
        assert_eq!(classify("#python"), TagKind::Freeform);
        assert_eq!(classify("#work/backend"), TagKind::Freeform);
        assert_eq!(classify("python"), TagKind::Freeform); // legacy bare name
        assert_eq!(classify(""), TagKind::Freeform);
    }

    #[test]
    fn bare_name_strips_prefix() {
        assert_eq!(bare_name("@home"), "home");
        assert_eq!(bare_name("$printer"), "printer");
        assert_eq!(bare_name("#python"), "python");
        assert_eq!(bare_name("#work/backend"), "work/backend");
        assert_eq!(bare_name("python"), "python");
    }

    // --- tag_matches ---

    #[test]
    fn exact_match() {
        assert!(tag_matches("#abc", "#abc"));
        assert!(tag_matches("@work", "@work"));
        assert!(tag_matches("#abc/cde", "#abc/cde"));
    }

    #[test]
    fn ancestor_matches_descendant() {
        assert!(tag_matches("#abc", "#abc/cde"));
        assert!(tag_matches("#abc/cde", "#abc/cde/fgh"));
        assert!(tag_matches("@work", "@work/frontend"));
    }

    #[test]
    fn partial_segment_does_not_match() {
        assert!(!tag_matches("#abc/cd", "#abc/cde")); // "cd" is not a full segment of "cde"
        assert!(!tag_matches("#ab", "#abc/cde")); // "ab" is not a full segment of "abc"
        assert!(!tag_matches("#abc/cde", "#abc/cd")); // filter is longer than tag
    }

    #[test]
    fn different_prefix_does_not_match() {
        assert!(!tag_matches("#abc", "@abc")); // wrong kind
        assert!(!tag_matches("@abc", "#abc"));
    }

    #[test]
    fn no_false_cross_segment_match() {
        assert!(!tag_matches("#abc", "#abcdef"));
    }

    #[test]
    fn resource_hierarchy() {
        assert!(tag_matches("$office", "$office/printer"));
        assert!(tag_matches("$office/printer", "$office/printer/color"));
        assert!(!tag_matches("$office", "$officedesk")); // no slash boundary
        assert!(!tag_matches("$office/desk", "$office/printer")); // sibling
    }

    // --- ancestors ---

    #[test]
    fn ancestors_single_segment() {
        assert_eq!(ancestors("#abc"), vec!["#abc"]);
    }

    #[test]
    fn ancestors_two_segments() {
        assert_eq!(ancestors("#abc/cde"), vec!["#abc", "#abc/cde"]);
    }

    #[test]
    fn ancestors_three_segments() {
        assert_eq!(
            ancestors("#a/b/c"),
            vec!["#a", "#a/b", "#a/b/c"]
        );
    }

    #[test]
    fn ancestors_context_tag() {
        assert_eq!(ancestors("@work/frontend"), vec!["@work", "@work/frontend"]);
    }

    #[test]
    fn ancestors_bare_name() {
        assert_eq!(ancestors("python"), vec!["python"]);
    }
}
