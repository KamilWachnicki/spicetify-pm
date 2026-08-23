pub mod apply_hook;
pub mod cache_cmd;
pub mod info;
pub mod install;
pub mod list;
pub mod lock_cmd;
pub mod remove;
pub mod search;
pub mod self_update;
pub mod snippets;
pub mod theme_cmd;
pub mod update;

use crate::cli::ItemTypeArg;
use crate::errors::Result;
use crate::http::HttpClient;
use crate::ledger::LedgerEntry;
use crate::market::blacklist::{Blacklist, fetch_blacklist};
use crate::market::constants::RepoTopic;

/// Save the ledger and keep the lockfile in sync. Every mutating command
/// persists through here so `spicepm.lock` never goes stale.
pub(crate) fn persist_ledger(led: &mut crate::ledger::Ledger) -> Result<()> {
    use crate::lockfile::Lockfile;
    led.save()?;
    let snippets = crate::commands::snippets::enabled_keys(led);
    let lockfile = Lockfile::from_ledger(led, &snippets);
    lockfile.store(&crate::lockfile::default_path())
}

/// Remove untracked files from `Extensions/` (orphans left behind by manual
/// installs or renames). Returns the removed relative paths.
pub(crate) fn reconcile_extensions_dir(led: &crate::ledger::Ledger) -> Result<Vec<String>> {
    let dir = crate::spicetify::dirs::extensions_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let tracked: std::collections::HashSet<String> = led
        .entries
        .iter()
        .flat_map(|e| e.files.iter().cloned())
        .collect();
    let base = crate::spicetify::dirs::spicetify_dir();
    let mut removed = Vec::new();
    for e in std::fs::read_dir(&dir)?.flatten() {
        let path = e.path();
        if !path.is_file() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(&base) else {
            continue;
        };
        let rel = rel.display().to_string();
        if tracked.contains(&rel)
            || path.file_name().is_some_and(|n| {
                n == std::ffi::OsStr::new(crate::commands::snippets::COMPANION_FILENAME)
            })
        {
            continue; // tracked, or the auto-generated companion
        }
        std::fs::remove_file(&path)?;
        crate::ui::info(format!("removed orphaned {rel}"));
        removed.push(rel);
    }
    Ok(removed)
}

/// The colour scheme currently applied to this theme entry, when it is the
/// active theme - captured into lockfiles so reinstalls restore it.
pub fn active_theme_scheme(entry: &LedgerEntry) -> Option<String> {
    let folder = entry.config_refs.first()?;
    let cfg =
        crate::spicetify::ini::SpicetifyConfig::load(crate::spicetify::dirs::config_file()).ok()?;
    if cfg.current_theme().as_deref() == Some(folder.as_str()) {
        cfg.color_scheme().filter(|s| !s.is_empty())
    } else {
        None
    }
}

/// Fetch and build the blacklist (shared by search/info/install).
pub async fn load_blacklist(http: &HttpClient) -> Result<Blacklist> {
    fetch_blacklist(http, crate::market::constants::BLACKLIST_TTL_SECS).await
}

pub fn topic_from_type(kind: Option<ItemTypeArg>) -> Option<RepoTopic> {
    match kind {
        Some(ItemTypeArg::Extension) => Some(RepoTopic::Extensions),
        Some(ItemTypeArg::Theme) => Some(RepoTopic::Themes),
        None => None,
    }
}
