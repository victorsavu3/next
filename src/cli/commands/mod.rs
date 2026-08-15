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
            let message = reject_misplaced_flags::<list::Args>(&tokens, "next list")
                .expect_err(&format!("{token} is a flag of `next list`"))
                .to_string();
            assert!(
                message.contains("before the filter expression"),
                "{token}: {message}"
            );
        }
        // …and a tag exclusion still is not a flag.
        assert!(reject_misplaced_flags::<list::Args>(&["-bug".to_string()], "next list").is_ok());
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
