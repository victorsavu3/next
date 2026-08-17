/// How a listing prints its results.
///
/// `--json` predates this and stays as an alias rather than a second concept:
/// scripts depend on it, and `--format json` is the same request spelled the
/// way a future third format would be. There is no third format — table and
/// json are the whole list, deliberately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
}

impl OutputFormat {
    /// Resolves the `--format` / `--json` pair into one answer.
    pub fn resolve(format: Option<OutputFormat>, json_alias: bool) -> OutputFormat {
        match format {
            Some(f) => f,
            None if json_alias => OutputFormat::Json,
            None => OutputFormat::Table,
        }
    }

    pub fn is_json(self) -> bool {
        self == OutputFormat::Json
    }
}

/// Refuses trailing tokens that are really `T`'s own flags.
///
/// The flag names come from clap itself, so a flag added to the `Args` struct
/// is known here the moment it exists — the alternative, a list written out
/// beside the check, is how `--fields` came to be reported as "unrecognised"
/// by the very command that documents it.
///
/// `command` is the command as typed, e.g. `next list`; it appears in the
/// message and in the `--help` pointer.
pub fn reject_misplaced_flags<T: clap::Args>(
    tokens: &[String],
    command: &str,
    trailing: crate::core::Trailing,
) -> anyhow::Result<()> {
    if tokens.is_empty() {
        return Ok(());
    }
    let mut cmd = T::augment_args(clap::Command::new("cmd"));
    // Builds the implicit args (`--help`) too, so the one flag nobody declares
    // is not the one that slips through.
    cmd.build();
    let mut long: Vec<String> = Vec::new();
    let mut short: Vec<char> = Vec::new();
    for arg in cmd.get_arguments() {
        long.extend(arg.get_long().map(str::to_owned));
        long.extend(
            arg.get_all_aliases()
                .into_iter()
                .flatten()
                .map(str::to_owned),
        );
        short.extend(arg.get_short());
        short.extend(arg.get_all_short_aliases().into_iter().flatten());
    }
    crate::core::reject_flag_like_tokens(
        tokens,
        &crate::core::KnownFlags::new(long, short),
        command,
        trailing,
    )
}

/// Refuses `--fields` where there is no JSON object to project.
///
/// The table prints a fixed set of columns and always did; `--fields` used to
/// be parsed and then dropped on that path, which is a flag that takes input
/// and does nothing — the kind of thing people file bugs about rather than
/// notice. `--fields` is new, so refusing is still cheap and is the honest
/// answer.
pub fn reject_fields_without_json(fields: &[String], json: bool) -> anyhow::Result<()> {
    if !fields.is_empty() && !json {
        anyhow::bail!(
            "`--fields` applies to JSON output; add `--json` (the table prints a fixed \
             set of columns)"
        );
    }
    Ok(())
}

/// Refuses `--fields` alongside `--count`, which prints no tasks to project.
///
/// `--json --count --fields id` passed the JSON guard above and then printed a
/// bare number, dropping the projection — the same silent no-op
/// [`reject_fields_without_json`] exists to prevent, reached by the one route
/// that guard does not cover. The two flags ask for different answers, so the
/// honest response is to say which one was meant.
pub fn reject_fields_with_count(fields: &[String], count: bool) -> anyhow::Result<()> {
    if !fields.is_empty() && count {
        anyhow::bail!(
            "`--fields` and `--count` ask for different things; `--count` prints only a \
             number, with no task to project"
        );
    }
    Ok(())
}

/// Refuses the output flags `--explain` would ignore.
///
/// `--explain` answers "what did my filter become?", and the answer is prose
/// aimed at a person: it names the terms that were read as searches, the view
/// terms in force, and where the work happened. **There is deliberately no
/// JSON form of it** — a machine-readable explanation would be a second
/// contract to keep in step with the pipeline, and the module's whole premise
/// is that an explanation which can disagree with the pipeline is worse than
/// none.
///
/// So `--json`, `--format json`, `--fields` and `--count` have nothing to act
/// on here, and `--explain` used to win silently over all four. Saying which
/// flag was meant is the same courtesy [`reject_fields_with_count`] extends.
pub fn reject_explain_with_output_flags(
    fields: &[String],
    json: bool,
    count: bool,
    explain: bool,
) -> anyhow::Result<()> {
    if !explain {
        return Ok(());
    }
    let ignored = if json {
        "`--json`"
    } else if !fields.is_empty() {
        "`--fields`"
    } else if count {
        "`--count`"
    } else {
        return Ok(());
    };
    anyhow::bail!(
        "`--explain` prints a prose explanation, so {ignored} has nothing to act on; \
         drop one of the two"
    )
}

pub mod add;
pub mod archive;
pub mod cancel;
pub mod config;
pub mod data;
pub mod delete;
pub mod done;
pub mod edit;
pub mod forecast;
pub mod init;
pub mod list;
pub mod maintenance;
pub mod move_cmd;
pub mod next_cmd;
pub mod open;
pub mod plugin;
pub mod show;
pub mod start;
pub mod stop;
pub mod sync;
pub mod tag;
pub mod tree;
pub mod tutorial;
pub mod user;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_known_flags_come_from_the_command_itself() {
        // Not a list maintained here: these are read off `list::Args`, so a
        // flag renamed there is caught here without anyone remembering to.
        for token in ["--fields", "--page-size", "--json", "-n", "--help"] {
            let tokens = vec![token.to_string()];
            let message = reject_misplaced_flags::<list::Args>(
                &tokens,
                "next list",
                crate::core::Trailing::Filter,
            )
            .expect_err(&format!("{token} is a flag of `next list`"))
            .to_string();
            assert!(
                message.contains("before the filter expression"),
                "{token}: {message}"
            );
        }
        // …and a tag exclusion still is not a flag.
        assert!(reject_misplaced_flags::<list::Args>(
            &["-bug".to_string()],
            "next list",
            crate::core::Trailing::Filter
        )
        .is_ok());
    }

    #[test]
    fn fields_needs_json() {
        let fields = ["id".to_string()];
        let message = reject_fields_without_json(&fields, false)
            .expect_err("a projection with no JSON to project is a usage error")
            .to_string();
        assert!(message.contains("--json"), "it names the fix: {message}");
        assert!(reject_fields_without_json(&fields, true).is_ok());
        // No projection asked for, nothing to complain about.
        assert!(reject_fields_without_json(&[], false).is_ok());
    }
}
