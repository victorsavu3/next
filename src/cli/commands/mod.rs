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
