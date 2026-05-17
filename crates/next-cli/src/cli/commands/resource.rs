use crate::AppContext;

/// Availability toggle for `next resource set`.
#[derive(clap::ValueEnum, Clone, Debug)]
pub enum Availability {
    On,
    Off,
}

/// Top-level `next resource` subcommand.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    #[command(subcommand)]
    pub subcommand: Option<ResourceSubcommand>,
}

#[derive(clap::Subcommand, Debug)]
pub enum ResourceSubcommand {
    /// Set availability of a $-prefixed resource tag.
    Set(SetArgs),
}

#[derive(clap::Args, Debug)]
pub struct SetArgs {
    /// Resource tag (e.g. $printer or $office/printer).
    pub resource: String,

    /// Whether the resource is currently available.
    pub availability: Availability,
}

pub fn run(args: Args, ctx: &mut AppContext) -> anyhow::Result<()> {
    match args.subcommand {
        None => show(ctx, args.json),
        Some(ResourceSubcommand::Set(a)) => set(ctx, a.resource, a.availability),
    }
}

fn show(ctx: &mut AppContext, json: bool) -> anyhow::Result<()> {
    let state = ctx.store.get_state()?;
    if json {
        // Emit a simple JSON object keyed by resource name.
        let mut entries: Vec<String> = state
            .resources
            .iter()
            .map(|(k, v)| format!("  \"${k}\": {v}"))
            .collect();
        entries.sort();
        println!("{{\n{}\n}}", entries.join(",\n"));
    } else if state.resources.is_empty() {
        println!("No resources tracked (all implicitly available).");
    } else {
        let mut rows: Vec<(&String, &bool)> = state.resources.iter().collect();
        rows.sort_by_key(|(k, _)| *k);
        for (name, available) in rows {
            let status = if *available { "available" } else { "unavailable" };
            println!("  ${name:<20} {status}");
        }
    }
    Ok(())
}

fn set(ctx: &mut AppContext, resource: String, availability: Availability) -> anyhow::Result<()> {
    let resource = if resource.starts_with('$') {
        resource
    } else {
        anyhow::bail!("resource tags must start with '$', got: {resource}");
    };

    let available = matches!(availability, Availability::On);
    let bare = resource.trim_start_matches('$');

    let mut state = ctx.store.get_state()?;
    state.resources.insert(bare.to_owned(), available);
    ctx.store.save_state(&state)?;

    let state_path = ctx.repo_root.join("state.toml");
    let status_word = if available { "on" } else { "off" };
    ctx.vcs.commit(
        &[state_path],
        &format!("next: resource {resource} {status_word}"),
    )?;

    let label = if available { "available" } else { "unavailable" };
    println!("{resource} marked as {label}.");
    Ok(())
}
