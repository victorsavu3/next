use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::core::domain::task::Priority;

/// Metadata stored for a tag in `tags/<tag>.toml`.
///
/// All fields are optional so the file format is forward-compatible: an
/// existing file with only `description = "…"` parses as a `TagMeta` with
/// the other fields absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TagMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Reference URL for this tag (e.g. a wiki page or issue tracker query).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Arbitrary key/value data attached to this tag.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub data: HashMap<String, JsonValue>,
    /// Default priority applied to tasks that carry this tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// When `true`, age and due-date proximity factors are zeroed out for
    /// tasks carrying this tag, preventing them from gaining urgency over time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_time_urgency: bool,
}

/// Classifies a tag string by its prefix convention.
///
/// All three kinds support hierarchical nesting via `/` separators.
/// A filter on a parent segment matches any descendant: `@work` matches
/// `@work/frontend`, `#office` matches `#office/printer`. Use [`tag_matches`]
/// to test this relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    /// `@name` or `@parent/child` — working-environment context (e.g. `@home`, `@work/frontend`).
    Context,
    /// `#name` or `#parent/child` — physical or situational resource (e.g. `#printer`, `#office/desk`).
    /// If a parent resource is unavailable, all its descendants are implicitly unavailable too.
    Resource,
    /// Bare name with no prefix — freeform hierarchical label (e.g. `python`, `lang/rust`).
    Freeform,
}

/// Returns the kind of a tag based on its prefix.
pub fn classify(tag: &str) -> TagKind {
    if tag.starts_with('@') {
        TagKind::Context
    } else if tag.starts_with('#') {
        TagKind::Resource
    } else {
        TagKind::Freeform
    }
}

/// Returns `true` if `tag` is a context tag (`@` prefix).
pub fn is_context(tag: &str) -> bool {
    matches!(classify(tag), TagKind::Context)
}

/// Returns `true` if `tag` is a resource tag (`#` prefix).
pub fn is_resource(tag: &str) -> bool {
    matches!(classify(tag), TagKind::Resource)
}

/// Returns the deepest context tag(s) from `tags`.
///
/// "Deepest" means having the most `/`-separated segments. When multiple tags
/// tie for depth, all of them are returned, sorted alphabetically. Returns an
/// empty vec when no context tags are present.
pub fn deepest_context_tags(tags: &[String]) -> Vec<String> {
    let ctx_tags: Vec<&str> = tags
        .iter()
        .filter(|t| is_context(t))
        .map(|s| s.as_str())
        .collect();
    if ctx_tags.is_empty() {
        return Vec::new();
    }
    let max_depth = ctx_tags
        .iter()
        .map(|t| t.matches('/').count())
        .max()
        .unwrap_or(0);
    let mut result: Vec<String> = ctx_tags
        .iter()
        .filter(|t| t.matches('/').count() == max_depth)
        .map(|s| s.to_string())
        .collect();
    result.sort();
    result
}

/// Strips the leading `@` or `#` prefix and returns the bare name.
///
/// For hierarchical tags the full path is returned: `bare_name("#office/printer") == "office/printer"`.
pub fn bare_name(tag: &str) -> &str {
    tag.trim_start_matches(['@', '#'])
}

/// Returns `true` when `filter` matches `tag`.
///
/// The same rules apply to all prefix kinds (`@`, `#`, bare):
/// - Exact match: `filter == tag`.
/// - Ancestor match: `tag` starts with `filter` followed immediately by `/`.
///
/// Examples:
/// ```
/// use next::core::domain::tag::tag_matches;
/// assert!(tag_matches("abc",       "abc"));           // exact bare
/// assert!(tag_matches("abc",       "abc/cde"));       // ancestor bare
/// assert!(tag_matches("@work",     "@work/frontend")); // context hierarchy
/// assert!(tag_matches("#office",   "#office/desk"));   // resource hierarchy
/// assert!(!tag_matches("abc/cd",   "abc/cde"));      // partial segment
/// assert!(!tag_matches("ab",       "abc/cde"));      // different segment
/// assert!(!tag_matches("abc",      "abcdef"));        // no slash boundary
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

/// Validates a tag string. Returns an `Err` with a description if the tag is malformed.
///
/// Rules applied to the name part (after stripping any `@` or `#` prefix), split by `/`:
/// - Each segment must start with an ASCII letter.
/// - Subsequent characters may be ASCII letters, digits, `-`, or `_`.
/// - Empty segments (e.g. trailing `/` or `//`) are rejected.
///
/// Context (`@`) and resource (`#`) tags follow the same rules for their name part.
/// Validates a tag string using an allowlist approach.
///
/// Allowed structure:
/// - Optional prefix: `@` (context) or `#` (resource)
/// - One or more segments separated by `/`
/// - Each segment: starts with an ASCII letter; remaining characters may be
///   letters (`a-z`, `A-Z`), digits (`0-9`), hyphen (`-`), or underscore (`_`)
/// - `..` is explicitly rejected as a segment to prevent path traversal
///   (tags are stored as `tags/<tag>.toml` on the filesystem)
pub fn validate_tag(tag: &str) -> Result<(), String> {
    let name = bare_name(tag);
    if name.is_empty() {
        return Err(format!("tag {tag:?} has an empty name after the prefix"));
    }
    for segment in name.split('/') {
        if segment.is_empty() {
            return Err(format!("tag {tag:?} contains an empty path segment"));
        }
        // Explicit guard: '..' as a segment would be a path traversal when tags
        // are stored as files under tags/<tag>.toml.
        if segment == ".." {
            return Err(format!("tag {tag:?} must not contain '..' path components"));
        }
        // '__' prefix is reserved for internal filesystem encoding.
        if segment.starts_with("__") {
            return Err(format!(
                "tag {tag:?}: segments must not start with '__' (reserved for internal use)"
            ));
        }
        let mut chars = segment.chars();
        let first = chars.next().unwrap();
        if !first.is_ascii_alphabetic() {
            return Err(format!(
                "tag {tag:?}: each segment must start with a letter, got {first:?}"
            ));
        }
        for c in chars {
            if !c.is_ascii_alphanumeric() && c != '_' && c != '-' {
                return Err(format!(
                    "tag {tag:?}: segments may only contain letters, digits, '-', or '_', got {c:?}"
                ));
            }
        }
    }
    Ok(())
}

/// Validates that `tag` is a well-formed context tag (must start with `@`).
///
/// Enforces the `@` prefix requirement and then applies the same segment rules
/// as [`validate_tag`].
pub fn validate_context_tag(tag: &str) -> Result<(), String> {
    if !tag.starts_with('@') {
        return Err(format!(
            "context tags must start with '@', got: {tag:?}"
        ));
    }
    validate_tag(tag)
}

/// Validates that `tag` is a well-formed resource tag (must start with `#`).
///
/// Enforces the `#` prefix requirement and then applies the same segment rules
/// as [`validate_tag`].
pub fn validate_resource_tag(tag: &str) -> Result<(), String> {
    if !tag.starts_with('#') {
        return Err(format!(
            "resource tags must start with '#', got: {tag:?}"
        ));
    }
    validate_tag(tag)
}

/// Returns all ancestor tag strings for `tag`, from outermost to `tag` itself.
///
/// For `#a/b/c` returns `["#a", "#a/b", "#a/b/c"]`. The prefix character is
/// preserved. For a tag with no `/`, returns only the tag itself.
pub fn ancestors(tag: &str) -> Vec<&str> {
    let (prefix, rest) = if let Some(s) = tag.strip_prefix(['@', '#']) {
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
        assert_eq!(classify("#printer"), TagKind::Resource);
        assert_eq!(classify("#vacation"), TagKind::Resource);
        assert_eq!(classify("#office/desk"), TagKind::Resource);
    }

    #[test]
    fn freeform_tag() {
        assert_eq!(classify("python"), TagKind::Freeform);
        assert_eq!(classify("work/backend"), TagKind::Freeform);
        assert_eq!(classify(""), TagKind::Freeform);
    }

    #[test]
    fn bare_name_strips_prefix() {
        assert_eq!(bare_name("@home"), "home");
        assert_eq!(bare_name("#printer"), "printer");
        assert_eq!(bare_name("#office/desk"), "office/desk");
        assert_eq!(bare_name("python"), "python");
    }

    // --- tag_matches ---

    #[test]
    fn exact_match() {
        assert!(tag_matches("abc", "abc"));
        assert!(tag_matches("@work", "@work"));
        assert!(tag_matches("abc/cde", "abc/cde"));
    }

    #[test]
    fn ancestor_matches_descendant() {
        assert!(tag_matches("abc", "abc/cde"));
        assert!(tag_matches("abc/cde", "abc/cde/fgh"));
        assert!(tag_matches("@work", "@work/frontend"));
    }

    #[test]
    fn partial_segment_does_not_match() {
        assert!(!tag_matches("abc/cd", "abc/cde")); // "cd" is not a full segment of "cde"
        assert!(!tag_matches("ab", "abc/cde")); // "ab" is not a full segment of "abc"
        assert!(!tag_matches("abc/cde", "abc/cd")); // filter is longer than tag
    }

    #[test]
    fn different_prefix_does_not_match() {
        assert!(!tag_matches("#abc", "@abc")); // wrong kind
        assert!(!tag_matches("@abc", "#abc"));
    }

    #[test]
    fn no_false_cross_segment_match() {
        assert!(!tag_matches("abc", "abcdef"));
    }

    #[test]
    fn resource_hierarchy() {
        assert!(tag_matches("#office", "#office/printer"));
        assert!(tag_matches("#office/printer", "#office/printer/color"));
        assert!(!tag_matches("#office", "#officedesk")); // no slash boundary
        assert!(!tag_matches("#office/desk", "#office/printer")); // sibling
    }

    // --- ancestors ---

    #[test]
    fn ancestors_single_segment() {
        assert_eq!(ancestors("abc"), vec!["abc"]);
    }

    #[test]
    fn ancestors_two_segments() {
        assert_eq!(ancestors("abc/cde"), vec!["abc", "abc/cde"]);
    }

    #[test]
    fn ancestors_three_segments() {
        assert_eq!(
            ancestors("a/b/c"),
            vec!["a", "a/b", "a/b/c"]
        );
    }

    #[test]
    fn ancestors_context_tag() {
        assert_eq!(ancestors("@work/frontend"), vec!["@work", "@work/frontend"]);
    }

    #[test]
    fn ancestors_resource_tag() {
        assert_eq!(ancestors("#office/printer"), vec!["#office", "#office/printer"]);
    }

    #[test]
    fn ancestors_bare_name() {
        assert_eq!(ancestors("python"), vec!["python"]);
    }

    // --- validate_tag ---

    #[test]
    fn valid_tags_pass_validation() {
        assert!(validate_tag("python").is_ok());
        assert!(validate_tag("lang-rust").is_ok());
        assert!(validate_tag("lang_rust").is_ok());
        assert!(validate_tag("@home").is_ok());
        assert!(validate_tag("#printer").is_ok());
        assert!(validate_tag("work/backend").is_ok());
        assert!(validate_tag("@work/front-end").is_ok());
        assert!(validate_tag("#office/printer").is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        assert!(validate_tag("@").is_err());
        assert!(validate_tag("#").is_err());
        assert!(validate_tag("").is_err());
    }

    #[test]
    fn segment_must_start_with_letter() {
        assert!(validate_tag("1task").is_err());
        assert!(validate_tag("_task").is_err());
        assert!(validate_tag("-task").is_err());
    }

    #[test]
    fn invalid_characters_rejected() {
        assert!(validate_tag("ta!g").is_err());
        assert!(validate_tag("ta g").is_err());
        assert!(validate_tag("ta@g").is_err());
    }

    #[test]
    fn empty_path_segment_rejected() {
        assert!(validate_tag("work//backend").is_err());
        assert!(validate_tag("work/").is_err());
    }

    #[test]
    fn double_underscore_prefix_rejected() {
        assert!(validate_tag("__reserved").is_err());
        assert!(validate_tag("@__reserved").is_err());
        assert!(validate_tag("#__reserved").is_err());
        assert!(validate_tag("work/__internal").is_err());
    }

    // --- validate_context_tag ---

    #[test]
    fn context_tag_valid() {
        assert!(validate_context_tag("@home").is_ok());
        assert!(validate_context_tag("@work").is_ok());
        assert!(validate_context_tag("@work/frontend").is_ok());
        assert!(validate_context_tag("@work/front-end").is_ok());
    }

    #[test]
    fn context_tag_missing_prefix_rejected() {
        assert!(validate_context_tag("home").is_err());
        assert!(validate_context_tag("#home").is_err());
        assert!(validate_context_tag("").is_err());
    }

    #[test]
    fn context_tag_segment_rules_enforced() {
        assert!(validate_context_tag("@").is_err());
        assert!(validate_context_tag("@1task").is_err());
        assert!(validate_context_tag("@work//backend").is_err());
        assert!(validate_context_tag("@__reserved").is_err());
    }

    // --- validate_resource_tag ---

    #[test]
    fn resource_tag_valid() {
        assert!(validate_resource_tag("#printer").is_ok());
        assert!(validate_resource_tag("#office").is_ok());
        assert!(validate_resource_tag("#office/printer").is_ok());
        assert!(validate_resource_tag("#office/color-printer").is_ok());
    }

    #[test]
    fn resource_tag_missing_prefix_rejected() {
        assert!(validate_resource_tag("printer").is_err());
        assert!(validate_resource_tag("@printer").is_err());
        assert!(validate_resource_tag("").is_err());
    }

    #[test]
    fn resource_tag_segment_rules_enforced() {
        assert!(validate_resource_tag("#").is_err());
        assert!(validate_resource_tag("#1printer").is_err());
        assert!(validate_resource_tag("#office//desk").is_err());
        assert!(validate_resource_tag("#__reserved").is_err());
    }
}
