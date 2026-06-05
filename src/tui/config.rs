//! TUI configuration loading.
//!
//! The TUI reuses the shared [`Config`] struct but loads its OWN config file,
//! `tui.toml`, falling back to the CLI's `config.toml` when `tui.toml` is
//! absent, then to [`Config::default`].

use std::path::{Path, PathBuf};

use crate::core::bootstrap;
use crate::Config;

/// Which file the loaded [`Config`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// Loaded from `tui.toml`.
    Tui,
    /// Loaded from the CLI's `config.toml` (no `tui.toml` present).
    Cli,
    /// No config file found — built-in defaults.
    Default,
}

impl ConfigSource {
    /// A short human-readable label, e.g. for the footer.
    pub fn label(self) -> &'static str {
        match self {
            ConfigSource::Tui => "tui.toml",
            ConfigSource::Cli => "config.toml",
            ConfigSource::Default => "defaults",
        }
    }
}

/// The directory holding the config files: `$XDG_CONFIG_HOME/task-manager`.
fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("task-manager"))
}

/// Loads the TUI config, applying the `tui.toml` → `config.toml` → defaults
/// fallback. Returns the config and which source was used.
///
/// When `config_override` is `Some`, that file is used directly and the source
/// is reported as [`ConfigSource::Tui`] (it stands in for the TUI's own file).
pub fn load(config_override: Option<&Path>) -> (Config, ConfigSource) {
    if let Some(path) = config_override {
        return (bootstrap::parse_config_file(path), ConfigSource::Tui);
    }
    match config_dir() {
        Some(dir) => load_from_dir(&dir),
        None => (Config::default(), ConfigSource::Default),
    }
}

/// The testable core of [`load`]: resolve the fallback against an explicit base
/// directory, without touching the environment.
fn load_from_dir(dir: &Path) -> (Config, ConfigSource) {
    let tui = dir.join("tui.toml");
    if tui.exists() {
        return (bootstrap::parse_config_file(&tui), ConfigSource::Tui);
    }
    let cli = dir.join("config.toml");
    if cli.exists() {
        return (bootstrap::parse_config_file(&cli), ConfigSource::Cli);
    }
    (Config::default(), ConfigSource::Default)
}

/// Composes [`load`] with repository resolution + opening.
///
/// * `repo_override` — use this repository root instead of the config value or
///   the upward directory search.
/// * `config_override` — use this config file instead of the `tui.toml` search.
pub fn bootstrap(
    repo_override: Option<&Path>,
    config_override: Option<&Path>,
) -> anyhow::Result<(Config, ConfigSource, crate::core::TaskRepository)> {
    let (config, source) = load(config_override);
    let root = bootstrap::resolve_root(repo_override, &config)?;
    let repo = bootstrap::open_repository(root, &config)?;
    Ok((config, source, repo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn tui_toml_wins_when_present() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("tui.toml"), "list_limit = 11\n").unwrap();
        fs::write(dir.path().join("config.toml"), "list_limit = 22\n").unwrap();

        let (cfg, source) = load_from_dir(dir.path());
        assert_eq!(source, ConfigSource::Tui);
        assert_eq!(cfg.list_limit, Some(11));
    }

    #[test]
    fn config_toml_used_when_tui_absent() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("config.toml"), "list_limit = 22\n").unwrap();

        let (cfg, source) = load_from_dir(dir.path());
        assert_eq!(source, ConfigSource::Cli);
        assert_eq!(cfg.list_limit, Some(22));
    }

    #[test]
    fn defaults_when_neither_present() {
        let dir = tempdir().unwrap();

        let (cfg, source) = load_from_dir(dir.path());
        assert_eq!(source, ConfigSource::Default);
        assert_eq!(cfg.list_limit, Config::default().list_limit);
    }

    #[test]
    fn repository_field_round_trips_from_chosen_file() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("tui.toml"),
            "repository = \"/tmp/tasks-from-tui\"\n",
        )
        .unwrap();

        let (cfg, source) = load_from_dir(dir.path());
        assert_eq!(source, ConfigSource::Tui);
        assert_eq!(cfg.repository, Some(PathBuf::from("/tmp/tasks-from-tui")));
    }
}
