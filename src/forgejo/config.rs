//! Configuration for the Forgejo plugin.
//!
//! Loaded from `~/.config/next-forgejo/config.toml`. Holds the Forgejo
//! connection and the repository → context mappings that drive imports.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Base URL of the Forgejo instance, e.g. `https://forgejo.victorsavu.eu`.
    #[serde(default)]
    pub forgejo_url: String,
    /// API token with issue read/write access.
    #[serde(default)]
    pub forgejo_token: String,
    /// The `next` repository to operate on for `sync`. When unset, the plugin
    /// falls back to `NEXT_REPO` (set by the export hook) or `next`'s own
    /// configured/default repository.
    #[serde(default)]
    pub next_repo: Option<PathBuf>,
    /// Forgejo repo → `next` context mappings.
    #[serde(default, rename = "map")]
    pub mappings: Vec<Mapping>,
}

/// Maps one Forgejo repository to the `next` context its issues import into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mapping {
    /// `owner/repo` on the Forgejo instance.
    pub repo: String,
    /// The `@context` tag imported tasks are tagged with.
    pub context: String,
}

impl Mapping {
    /// Splits `repo` into `(owner, repo)`.
    pub fn owner_repo(&self) -> Result<(&str, &str)> {
        self.repo
            .split_once('/')
            .filter(|(o, r)| !o.is_empty() && !r.is_empty())
            .with_context(|| format!("mapping repo {:?} must be \"owner/repo\"", self.repo))
    }
}

/// Default config path: `$XDG_CONFIG_HOME/next-forgejo/config.toml`.
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("next-forgejo")
        .join("config.toml")
}

/// Loads and validates the config from the default path.
pub fn load() -> Result<Config> {
    load_from(&config_path())
}

/// Loads and validates the config from `path` (separated for testing).
pub fn load_from(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read config {}", path.display()))?;
    let cfg: Config = toml::from_str(&text).context("parse next-forgejo config")?;
    anyhow::ensure!(!cfg.forgejo_url.is_empty(), "forgejo_url not set in {}", path.display());
    anyhow::ensure!(!cfg.forgejo_token.is_empty(), "forgejo_token not set in {}", path.display());
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parses_full_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write(
            &dir,
            r#"
                forgejo_url = "https://forgejo.example.com"
                forgejo_token = "secret"
                next_repo = "/home/u/tasks"

                [[map]]
                repo = "victor/task-manager"
                context = "@ai/task-manager"

                [[map]]
                repo = "victor/forgejo_claude"
                context = "@ai/forgejo_claude"
            "#,
        );
        let cfg = load_from(&path).unwrap();
        assert_eq!(cfg.forgejo_url, "https://forgejo.example.com");
        assert_eq!(cfg.next_repo, Some(PathBuf::from("/home/u/tasks")));
        assert_eq!(cfg.mappings.len(), 2);
        assert_eq!(cfg.mappings[0].owner_repo().unwrap(), ("victor", "task-manager"));
        assert_eq!(cfg.mappings[1].context, "@ai/forgejo_claude");
    }

    #[test]
    fn minimal_config_no_mappings() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write(&dir, "forgejo_url = \"https://f\"\nforgejo_token = \"t\"\n");
        let cfg = load_from(&path).unwrap();
        assert!(cfg.mappings.is_empty());
        assert!(cfg.next_repo.is_none());
    }

    #[test]
    fn missing_token_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write(&dir, "forgejo_url = \"https://f\"\n");
        let err = load_from(&path).unwrap_err();
        assert!(err.to_string().contains("forgejo_token"));
    }

    #[test]
    fn bad_repo_format_errors() {
        let m = Mapping { repo: "noslash".into(), context: "@x".into() };
        assert!(m.owner_repo().is_err());
        let m2 = Mapping { repo: "owner/".into(), context: "@x".into() };
        assert!(m2.owner_repo().is_err());
    }
}
