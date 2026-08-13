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
