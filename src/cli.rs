use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "spicepm",
    version,
    about = "Spicetify Marketplace package manager — themes, extensions and snippets",
    after_help = "Sources and filtering rules mirror the official Spicetify Marketplace."
)]
pub struct Cli {
    /// Fetch fresh data, bypassing the disk cache
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Run `spicetify apply` automatically after mutating commands
    #[arg(long, global = true)]
    pub apply: bool,

    /// Skip the administrator/root guard (NOT RECOMMENDED)
    #[arg(long, global = true)]
    pub bypass_admin: bool,

    /// Increase log verbosity (-v info, -vv debug)
    #[arg(short = 'v', long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemTypeArg {
    Extension,
    Theme,
}

/// Sort modes mirroring the marketplace dropdown.
#[derive(ValueEnum, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortArg {
    #[default]
    Stars,
    Newest,
    Oldest,
    LastUpdated,
    MostStale,
    #[value(name = "a-z")]
    Az,
    #[value(name = "z-a")]
    Za,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Search the marketplace by GitHub topic
    Search {
        /// Filter results by substring (name, description, repo, tags)
        query: Option<String>,
        /// Restrict to one item type
        #[arg(short, long)]
        r#type: Option<ItemTypeArg>,
        #[arg(short, long, default_value = "stars")]
        sort: SortArg,
        /// Only fetch this single results page (default: all pages)
        #[arg(long)]
        page: Option<u32>,
        /// Include archived repositories
        #[arg(long)]
        archived: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show manifests and metadata for a repository
    Info {
        /// `user/repo` or a github.com URL
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Install extensions/themes, or restore everything from spicepm.lock
    Install {
        /// `user/repo[#manifest-name]`, or a URL; omit to use the lockfile
        target: Option<String>,
        /// Assume defaults, skip prompts
        #[arg(short, long)]
        yes: bool,
        /// Lockfile path used when no target is given
        #[arg(long)]
        lockfile: Option<PathBuf>,
    },
    /// Write the current installed set to spicepm.lock
    Lock {
        /// Where to write the lockfile [default: ./spicepm.lock]
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Remove an installed extension/theme/snippet
    Uninstall {
        target: String,
        #[arg(short, long)]
        yes: bool,
    },
    /// List installed items
    #[command(subcommand)]
    List(ListCommands),
    /// Re-fetch and re-install an item (default: everything)
    Update {
        /// Item id or unique name fragment; omit for all
        target: Option<String>,
    },
    /// Browse and manage CSS snippets
    #[command(subcommand)]
    Snippets(SnippetCommands),
    /// Inspect and switch themes
    #[command(subcommand)]
    Theme(ThemeCommands),
    /// Manage cached responses
    #[command(subcommand)]
    Cache(CacheCommands),
}

#[derive(Subcommand, Debug)]
pub enum ListCommands {
    /// Items installed by spicepm
    Installed {
        #[arg(short, long)]
        r#type: Option<ItemTypeArg>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SnippetCommands {
    /// All available CSS snippets
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print a snippet's CSS
    Show { name: String },
    /// Enable a snippet (applies via the spicepm companion extension)
    Add { name: String },
    /// Disable a snippet
    Remove { name: String },
    /// Currently enabled snippets
    Installed {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ThemeCommands {
    /// Activate an installed theme by folder name
    Set {
        name: Option<String>,
        scheme: Option<String>,
    },
    /// Change the active colour scheme of the current theme
    Scheme { scheme: String },
    /// Show the active theme and scheme
    Current {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Print the cache directory
    Path,
    /// Delete all cached responses
    Clear,
}

/// Parse a `user/repo` pair or a github.com/raw.githubusercontent.com URL.
pub fn parse_target(target: &str) -> Option<(String, String)> {
    let cleaned = target
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();

    let rest = if let Some(r) = cleaned.strip_prefix("github.com/") {
        r.to_owned()
    } else if let Some(loc) = crate::market::urls::parse_github_raw(target) {
        format!("{}/{}", loc.user, loc.repo)
    } else if cleaned.starts_with("raw.githubusercontent.com/") {
        let parts: Vec<&str> = cleaned.split('/').collect();
        if parts.len() < 3 {
            return None;
        }
        format!("{}/{}", parts[1], parts[2])
    } else {
        cleaned.clone()
    };

    let mut segments = rest.split('/');
    let user = segments.next()?.to_owned();
    let repo = segments.next()?.to_owned();
    if user.is_empty() || repo.is_empty() {
        return None;
    }
    Some((user, repo))
}

/// Split `user/repo#manifest-name`.
pub fn split_manifest_selector(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((base, name)) => (base, Some(name)),
        None => (target, None),
    }
}
