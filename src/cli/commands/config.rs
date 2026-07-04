use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::Config;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub subcommand: ConfigSubcommand,
}

#[derive(clap::Subcommand, Debug)]
pub enum ConfigSubcommand {
    /// Print one or all config values.
    Get(GetArgs),
    /// Set a config value and save the file.
    Set(SetArgs),
}

#[derive(clap::Args, Debug)]
pub struct GetArgs {
    /// Config key to read (e.g. `autosync`, `sync.pull_before_query`).
    /// Omit to print all keys.
    pub key: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    /// Config key to set.
    pub key: String,
    /// New value.
    pub value: String,
}

pub fn run(args: Args, config_path: Option<&Path>) -> anyhow::Result<()> {
    match args.subcommand {
        ConfigSubcommand::Get(a) => cmd_get(a, config_path),
        ConfigSubcommand::Set(a) => cmd_set(a, config_path),
    }
}

fn resolve_config_path(override_path: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p.to_path_buf());
    }
    dirs::config_dir()
        .map(|d| d.join("task-manager").join("config.toml"))
        .context("cannot determine XDG config directory")
}

fn load_config(config_path: Option<&Path>) -> Config {
    match config_path {
        Some(p) => crate::core::bootstrap::parse_config_file(p),
        None => {
            let path = dirs::config_dir().map(|d| d.join("task-manager").join("config.toml"));
            match path {
                Some(p) => crate::core::bootstrap::parse_config_file(&p),
                None => Config::default(),
            }
        }
    }
}

fn print_key(cfg: &Config, key: &str) -> anyhow::Result<()> {
    match key {
        "autosync" => println!("autosync = {}", cfg.autosync),
        "sync.pull_before_query" => {
            println!("sync.pull_before_query = {}", cfg.sync.pull_before_query)
        }
        "sync.git_subprocess" => println!("sync.git_subprocess = {}", cfg.sync.git_subprocess),
        "sync.staleness_secs" => println!("sync.staleness_secs = {}", cfg.sync.staleness_secs),
        "sync.pull_timeout_secs" => {
            println!("sync.pull_timeout_secs = {}", cfg.sync.pull_timeout_secs)
        }
        "list_limit" => match cfg.list_limit {
            Some(v) => println!("list_limit = {v}"),
            None => println!("list_limit = none"),
        },
        "next_count" => println!("next_count = {}", cfg.next_count),
        "forecast_horizon_days" => {
            println!("forecast_horizon_days = {}", cfg.forecast_horizon_days)
        }
        "repository" => match &cfg.repository {
            Some(p) => println!("repository = {}", p.display()),
            None => println!("repository = none"),
        },
        _ => anyhow::bail!("unknown config key: {key}"),
    }
    Ok(())
}

fn print_all(cfg: &Config) -> anyhow::Result<()> {
    println!("autosync = {}", cfg.autosync);
    println!("sync.pull_before_query = {}", cfg.sync.pull_before_query);
    println!("sync.git_subprocess = {}", cfg.sync.git_subprocess);
    println!("sync.staleness_secs = {}", cfg.sync.staleness_secs);
    println!("sync.pull_timeout_secs = {}", cfg.sync.pull_timeout_secs);
    match cfg.list_limit {
        Some(v) => println!("list_limit = {v}"),
        None => println!("list_limit = none"),
    }
    println!("next_count = {}", cfg.next_count);
    println!("forecast_horizon_days = {}", cfg.forecast_horizon_days);
    match &cfg.repository {
        Some(p) => println!("repository = {}", p.display()),
        None => println!("repository = none"),
    }
    Ok(())
}

fn parse_bool(value: &str) -> anyhow::Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => anyhow::bail!("expected a boolean (true/false), got {value:?}"),
    }
}

fn apply_set(cfg: &mut Config, key: &str, value: &str) -> anyhow::Result<()> {
    match key {
        "autosync" => cfg.autosync = parse_bool(value)?,
        "sync.pull_before_query" => cfg.sync.pull_before_query = parse_bool(value)?,
        "sync.git_subprocess" => cfg.sync.git_subprocess = parse_bool(value)?,
        "sync.staleness_secs" => {
            cfg.sync.staleness_secs = value
                .parse::<u64>()
                .with_context(|| format!("sync.staleness_secs expects a u64, got {value:?}"))?;
        }
        "sync.pull_timeout_secs" => {
            cfg.sync.pull_timeout_secs = value
                .parse::<u64>()
                .with_context(|| format!("sync.pull_timeout_secs expects a u64, got {value:?}"))?;
        }
        "list_limit" => {
            if value.eq_ignore_ascii_case("none") {
                cfg.list_limit = None;
            } else {
                cfg.list_limit = Some(
                    value
                        .parse::<usize>()
                        .with_context(|| {
                            format!("list_limit expects a usize or 'none', got {value:?}")
                        })?,
                );
            }
        }
        "next_count" => {
            cfg.next_count = value
                .parse::<usize>()
                .with_context(|| format!("next_count expects a usize, got {value:?}"))?;
        }
        "forecast_horizon_days" => {
            cfg.forecast_horizon_days = value
                .parse::<u32>()
                .with_context(|| format!("forecast_horizon_days expects a u32, got {value:?}"))?;
        }
        "repository" => {
            if value.eq_ignore_ascii_case("none") {
                cfg.repository = None;
            } else {
                cfg.repository = Some(PathBuf::from(value));
            }
        }
        _ => anyhow::bail!("unknown config key: {key}"),
    }
    Ok(())
}

fn cmd_get(args: GetArgs, config_path: Option<&Path>) -> anyhow::Result<()> {
    let cfg = load_config(config_path);
    match args.key {
        Some(key) => print_key(&cfg, &key),
        None => print_all(&cfg),
    }
}

fn cmd_set(args: SetArgs, config_path: Option<&Path>) -> anyhow::Result<()> {
    let path = resolve_config_path(config_path)?;
    let mut cfg = crate::core::bootstrap::parse_config_file(&path);

    apply_set(&mut cfg, &args.key, &args.value)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create config directory {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(&cfg).context("failed to serialize config to TOML")?;
    std::fs::write(&path, text)
        .with_context(|| format!("cannot write config file {}", path.display()))?;

    print_key(&cfg, &args.key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn set_bool_autosync() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");

        cmd_set(SetArgs { key: "autosync".into(), value: "true".into() }, Some(&path)).unwrap();

        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert!(cfg.autosync);
    }

    #[test]
    fn set_nested_bool_pull_before_query() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");

        cmd_set(
            SetArgs { key: "sync.pull_before_query".into(), value: "false".into() },
            Some(&path),
        )
        .unwrap();

        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert!(!cfg.sync.pull_before_query);
    }

    #[test]
    fn get_single_key() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "autosync = true\n");

        let args = GetArgs { key: Some("autosync".into()) };
        cmd_get(args, Some(&path)).unwrap();
    }

    #[test]
    fn get_all_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");

        cmd_set(SetArgs { key: "next_count".into(), value: "5".into() }, Some(&path)).unwrap();
        cmd_set(
            SetArgs { key: "forecast_horizon_days".into(), value: "30".into() },
            Some(&path),
        )
        .unwrap();

        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert_eq!(cfg.next_count, 5);
        assert_eq!(cfg.forecast_horizon_days, 30);

        cmd_get(GetArgs { key: None }, Some(&path)).unwrap();
    }

    #[test]
    fn set_list_limit_and_clear_with_none() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");

        cmd_set(SetArgs { key: "list_limit".into(), value: "50".into() }, Some(&path)).unwrap();
        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert_eq!(cfg.list_limit, Some(50));

        cmd_set(SetArgs { key: "list_limit".into(), value: "none".into() }, Some(&path)).unwrap();
        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert_eq!(cfg.list_limit, None);
    }

    #[test]
    fn set_repository_and_clear() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");

        cmd_set(
            SetArgs { key: "repository".into(), value: "/tmp/tasks".into() },
            Some(&path),
        )
        .unwrap();
        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert_eq!(cfg.repository, Some(PathBuf::from("/tmp/tasks")));

        cmd_set(
            SetArgs { key: "repository".into(), value: "none".into() },
            Some(&path),
        )
        .unwrap();
        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert_eq!(cfg.repository, None);
    }

    #[test]
    fn unknown_key_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");
        let result = cmd_set(
            SetArgs { key: "nonexistent".into(), value: "x".into() },
            Some(&path),
        );
        assert!(result.is_err());
    }

    #[test]
    fn set_creates_missing_config_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("subdir").join("config.toml");

        cmd_set(
            SetArgs { key: "autosync".into(), value: "true".into() },
            Some(&path),
        )
        .unwrap();
        assert!(path.exists());
        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert!(cfg.autosync);
    }

    #[test]
    fn set_sync_u64_fields() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");

        cmd_set(
            SetArgs { key: "sync.staleness_secs".into(), value: "7200".into() },
            Some(&path),
        )
        .unwrap();
        cmd_set(
            SetArgs { key: "sync.pull_timeout_secs".into(), value: "30".into() },
            Some(&path),
        )
        .unwrap();

        let cfg = crate::core::bootstrap::parse_config_file(&path);
        assert_eq!(cfg.sync.staleness_secs, 7200);
        assert_eq!(cfg.sync.pull_timeout_secs, 30);
    }

    #[test]
    fn invalid_bool_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");
        let result = cmd_set(
            SetArgs { key: "autosync".into(), value: "maybe".into() },
            Some(&path),
        );
        assert!(result.is_err());
    }

    #[test]
    fn get_unknown_key_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "");
        let result = cmd_get(GetArgs { key: Some("no_such_key".into()) }, Some(&path));
        assert!(result.is_err());
    }
}
