//! Choosing which fields come back from a listing.
//!
//! The driver is payload size, not tidiness. `list_tasks` serialises every
//! field of every task — descriptions and notes included — and on a repository
//! whose notes run to thousands of words a single call can return six figures
//! of characters. The cost is per row, so pagination does not help.
//!
//! Two rules shape the design:
//!
//! - **One vocabulary.** Field names are the ones the filter grammar already
//!   uses ([`Field::name`]), so `due` means the same thing in `--fields due` as
//!   in `due<+7d`. Two spellings for one concept is how a tool becomes hard to
//!   learn, so the few extras (`id`, `score`) are the only additions. The
//!   vocabularies are not identical, though: `created` and
//!   `updated` are filterable but come from git history rather than the task
//!   object, so they are refused — with an explanation, not a "typo?" message.
//! - **Omit, do not null.** A field that was not asked for is ABSENT from the
//!   JSON object rather than present as `null`. `null` already means "this task
//!   has no due date", and a consumer cannot tell the two apart otherwise.
//!
//! Projection never affects which tasks match: `--fields id,title` with a
//! filter on `notes:x` still filters on notes. Selection and presentation are
//! different stages.

use serde_json::Value;

use crate::core::error::{Result, TaskError};

/// The set of fields a caller asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// JSON keys of the task object to keep.
    keys: Vec<String>,
    /// `data.<key>` entries to keep, when `data` itself was not asked for
    /// wholesale.
    data_keys: Vec<String>,
    /// Whether the whole `data` map was requested.
    all_data: bool,
    /// Whether the caller wants the computed score, which lives outside the
    /// task object.
    score: bool,
}

/// Every projectable name, paired with the JSON key it selects.
///
/// The left column is the filter grammar's vocabulary; the right is what serde
/// actually emits. They differ in three places, which is exactly why this table
/// exists rather than a `to_string` on the field name.
const FIELDS: &[(&str, &str)] = &[
    ("id", "id"),
    ("title", "title"),
    ("status", "status"),
    ("priority", "priority"),
    ("due", "due"),
    ("start", "start"),
    ("completed", "completed_at"),
    ("slug", "slug"),
    ("parent", "parent_id"),
    ("assignee", "assignee"),
    ("tags", "tags"),
    ("tag", "tags"),
    ("blocked_by", "blocked_by"),
    ("description", "description"),
    ("notes", "notes"),
    ("url", "url"),
    ("recurrence", "recurrence"),
    ("long_term", "long_term"),
    ("score_adjustment", "score_adjustment"),
    ("data", "data"),
];

/// Filter-grammar fields that no task object carries.
///
/// `created:` and `updated:` are read from the file's git history when a query
/// runs, so `created>2026-01-01` works while `--fields created` cannot: there
/// is no key to keep. Refusing them is right; refusing them as "unknown field"
/// is not, because it sends the reader looking for a typo in a name the filter
/// grammar accepts three lines further up the same command.
const GIT_DERIVED: &[&str] = &["created", "updated"];

/// The fields returned when a caller asks for none.
///
/// Everything, so that adding projection changes no existing response. Making
/// a lean set the default is a breaking change to every consumer and deserves
/// its own decision rather than arriving as a side effect of this feature.
pub const DEFAULT_IS_EVERYTHING: bool = true;

impl Projection {
    /// Parses a caller's field list.
    ///
    /// An unknown name is an error listing what is available — silently
    /// dropping it would return a response missing a field the caller believes
    /// they asked for, which is worse than failing. A name the *filter* knows
    /// but the task object does not ([`GIT_DERIVED`]) gets its own message, so
    /// the reader is told why rather than left to hunt for a typo.
    pub fn parse(names: &[String]) -> Result<Self> {
        let mut keys = Vec::new();
        let mut data_keys = Vec::new();
        let mut all_data = false;
        let mut score = false;

        for name in names {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if name == "score" {
                score = true;
                continue;
            }
            if GIT_DERIVED.contains(&name) {
                return Err(TaskError::Other(format!(
                    "{name:?} comes from git history, not the task object — you \
                     can filter on it ({name}>2026-01-01) but there is no field \
                     to return"
                )));
            }
            if let Some(key) = name.strip_prefix("data.") {
                if key.is_empty() {
                    return Err(TaskError::Other(
                        "data. needs a key, e.g. data.estimate".to_owned(),
                    ));
                }
                data_keys.push(key.to_owned());
                keys.push("data".to_owned());
                continue;
            }
            match FIELDS.iter().find(|(n, _)| *n == name) {
                Some((_, key)) => {
                    if *key == "data" {
                        all_data = true;
                    }
                    keys.push((*key).to_owned());
                }
                None => {
                    let mut known: Vec<&str> = FIELDS.iter().map(|(n, _)| *n).collect();
                    known.push("score");
                    known.push("data.<key>");
                    return Err(TaskError::Other(format!(
                        "unknown field {name:?} — expected one of {}",
                        known.join(", ")
                    )));
                }
            }
        }

        Ok(Self {
            keys,
            data_keys,
            all_data,
            score,
        })
    }

    /// Whether the caller asked for anything. An empty projection means "give
    /// me everything", so callers can skip the work entirely.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && !self.score
    }

    /// Trims a serialised task in place.
    pub fn apply_to_task(&self, task: &mut Value) {
        if self.is_empty() {
            return;
        }
        let Some(map) = task.as_object_mut() else {
            return;
        };
        map.retain(|k, _| self.keys.iter().any(|want| want == k));

        // `data.estimate` keeps one entry of the map, not the whole thing.
        if !self.all_data && !self.data_keys.is_empty() {
            if let Some(Value::Object(data)) = map.get_mut("data") {
                data.retain(|k, _| self.data_keys.iter().any(|want| want == k));
            }
        }
    }

    /// Trims a serialised `ScoredTask` — `{ "score": …, "task": { … } }` — in
    /// place, so `score` is projectable alongside the task's own fields.
    pub fn apply_to_scored(&self, scored: &mut Value) {
        if self.is_empty() {
            return;
        }
        let Some(map) = scored.as_object_mut() else {
            return;
        };
        if let Some(task) = map.get_mut("task") {
            self.apply_to_task(task);
        }
        map.retain(|k, _| k == "task" || (k == "score" && self.score));
    }

    /// Trims every item of a `Page` of scored tasks, leaving the pagination
    /// envelope alone — a caller projecting fields still needs to know there
    /// are more pages.
    pub fn apply_to_page(&self, page: &mut Value) {
        if self.is_empty() {
            return;
        }
        if let Some(items) = page.get_mut("items").and_then(|v| v.as_array_mut()) {
            for item in items {
                // A page carries scored tasks in the active tier and bare
                // tasks in the archived one.
                if item.get("task").is_some() {
                    self.apply_to_scored(item);
                } else {
                    self.apply_to_task(item);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::domain::task::Task;

    fn sample() -> Value {
        let mut t = Task::new("Rebuild the cache");
        t.slug = Some("rebuild".into());
        t.notes = Some("A very long note".into());
        t.description = Some("A description".into());
        t.assignee = Some("alice".into());
        t.tags = vec!["@work".into()];
        t.data.insert("estimate".into(), serde_json::json!(3));
        t.data.insert("owner".into(), serde_json::json!("platform"));
        serde_json::to_value(&t).unwrap()
    }

    fn proj(names: &[&str]) -> Projection {
        Projection::parse(&names.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn an_empty_projection_changes_nothing() {
        let mut task = sample();
        let before = task.clone();
        proj(&[]).apply_to_task(&mut task);
        assert_eq!(task, before, "no fields asked for means everything");
    }

    #[test]
    fn unrequested_fields_are_absent_not_null() {
        let mut task = sample();
        proj(&["id", "title"]).apply_to_task(&mut task);
        let map = task.as_object().unwrap();
        assert!(map.contains_key("id"));
        assert!(map.contains_key("title"));
        assert!(
            !map.contains_key("notes"),
            "an unrequested field must be ABSENT — null already means \
             'this task has no value here'"
        );
        assert_eq!(map.len(), 2, "{map:?}");
    }

    #[test]
    fn the_names_are_the_filter_grammars_names() {
        // `completed`, `parent` and `tag` are spelled the way a filter spells
        // them, not the way serde does.
        let mut task = sample();
        proj(&["completed", "parent", "tag"]).apply_to_task(&mut task);
        let map = task.as_object().unwrap();
        assert!(map.contains_key("tags"), "{map:?}");
        // completed_at and parent_id are absent on this task, so the mapping is
        // asserted through the parse rather than the output.
        let p = proj(&["completed", "parent"]);
        assert!(p.keys.contains(&"completed_at".to_owned()));
        assert!(p.keys.contains(&"parent_id".to_owned()));
    }

    #[test]
    fn a_single_data_key_does_not_bring_the_whole_map() {
        let mut task = sample();
        proj(&["data.estimate"]).apply_to_task(&mut task);
        let data = task.get("data").unwrap().as_object().unwrap();
        assert!(data.contains_key("estimate"));
        assert!(!data.contains_key("owner"), "{data:?}");
    }

    #[test]
    fn asking_for_data_wholesale_keeps_every_key() {
        let mut task = sample();
        proj(&["data"]).apply_to_task(&mut task);
        let data = task.get("data").unwrap().as_object().unwrap();
        assert_eq!(data.len(), 2, "{data:?}");
    }

    #[test]
    fn score_is_projectable_and_lives_outside_the_task() {
        let mut scored = serde_json::json!({ "score": 4.5, "task": sample() });
        proj(&["id", "score"]).apply_to_scored(&mut scored);
        assert!(scored.get("score").is_some());
        assert_eq!(scored["task"].as_object().unwrap().len(), 1);

        // Not asking for it drops it, which is the point — it is the field a
        // caller listing ids has least use for.
        let mut scored = serde_json::json!({ "score": 4.5, "task": sample() });
        proj(&["id"]).apply_to_scored(&mut scored);
        assert!(scored.get("score").is_none());
    }

    #[test]
    fn a_page_keeps_its_envelope() {
        let mut page = serde_json::json!({
            "items": [{ "score": 1.0, "task": sample() }],
            "page": 1, "page_size": 50, "total": 1
        });
        proj(&["id"]).apply_to_page(&mut page);
        assert_eq!(page["total"], 1, "pagination must survive projection");
        assert_eq!(page["items"][0]["task"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn an_unknown_field_is_refused_with_the_alternatives() {
        let err = Projection::parse(&["nosuchfield".to_owned()]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown field"), "{msg}");
        assert!(msg.contains("title"), "it lists what is available: {msg}");
        assert!(msg.contains("data.<key>"), "{msg}");

        assert!(Projection::parse(&["data.".to_owned()]).is_err());
    }

    #[test]
    fn a_git_derived_field_says_so_instead_of_unknown() {
        for name in GIT_DERIVED {
            let msg = Projection::parse(&[(*name).to_owned()])
                .unwrap_err()
                .to_string();
            let lower = msg.to_lowercase();
            assert!(lower.contains("git"), "{name}: {msg}");
            assert!(
                lower.contains("filter"),
                "{name}: the filter still takes it, and the message must say \
                 so: {msg}"
            );
            assert!(
                !msg.contains("unknown field"),
                "{name} is not a typo: {msg}"
            );
        }
    }
}
