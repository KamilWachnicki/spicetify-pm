use crate::errors::{Error, Result};
use crate::http::HttpClient;
use crate::ledger::{Kind, Ledger, LedgerEntry};
use crate::market::items::build_card_items;
use crate::market::search::get_repo;
use crate::market::types::CardItem;
use crate::spicetify::dirs;
use crate::spicetify::ini::SpicetifyConfig;
use crate::ui;

const FRESH_TTL: u64 = 0; // always refetch on update

pub async fn run(http: &HttpClient, target: Option<String>) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let led = Ledger::load()?;

    let targets: Vec<LedgerEntry> = match target {
        Some(ref t) => {
            let lower = t.to_lowercase();
            let found: Vec<LedgerEntry> = led
                .entries
                .iter()
                .filter(|e| e.id.to_lowercase().contains(&lower))
                .cloned()
                .collect();
            if found.is_empty() {
                return Err(Error::other(format!("no installed item matches `{t}`")));
            }
            found
        }
        None => led.entries.clone(),
    };

    let mut updated = 0usize;
    for entry in &targets {
        let result = match entry.kind {
            Kind::SnippetGroup => super::snippets::regenerate_companion(http, entry).await,
            Kind::Extension | Kind::Theme => update_item(http, entry).await,
        };
        if result? {
            updated += 1;
        }
    }

    if updated == 0 {
        ui::info("everything already up to date");
    } else {
        ui::success(format!("updated {updated} item(s)"));
        ui::reminder_apply(crate::commands::apply_hook::requested());
    }
    Ok(())
}

async fn update_item(http: &HttpClient, entry: &LedgerEntry) -> Result<bool> {
    let spinner = ui::spinner(false, format!("updating {}", entry.id));

    let repo = get_repo(http, &entry.user, &entry.repo).await?;
    let (manifests, warnings) =
        get_repo_manifests_fresh(http, &entry.user, &entry.repo, &repo.default_branch).await?;
    for warning in warnings {
        ui::warn(warning);
    }
    let items = build_card_items(&repo, manifests);
    let result = if let Some(item) = items.iter().find(|i| i.id() == entry.id) {
        match item.kind {
            crate::market::types::ItemKind::Extension => update_extension(http, entry, item).await,
            crate::market::types::ItemKind::Theme => update_theme(http, entry, item).await,
        }
    } else {
        ui::warn(format!(
            "`{}` no longer exists upstream (removed or renamed); uninstall and reinstall to migrate",
            entry.id
        ));
        Ok(false)
    };

    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }
    result
}

/// Extensions are a single file: redownload it, drop a renamed-away old
/// file, and keep the config registration pointing at the right basename.
async fn update_extension(http: &HttpClient, entry: &LedgerEntry, item: &CardItem) -> Result<bool> {
    let url = item
        .extension_url
        .clone()
        .ok_or_else(|| Error::other("manifest has no main"))?;
    let bytes = http.download(&url).await?;
    let new_basename =
        crate::commands::install::file_name_from(item.manifest.main.as_deref().unwrap_or_default());
    let new_rel = format!("Extensions/{new_basename}");
    let new_path = crate::ledger::resolve_relative(&new_rel)?;
    let new_hash = crate::ledger::hash_bytes(&bytes);

    let mut changed = false;
    if entry.hashes.get(&new_rel).is_some_and(|h| *h != new_hash)
        || !new_path.is_file()
        || entry.files != [new_rel.as_str()]
    {
        changed = true;
    }

    crate::cache::atomic_write(&new_path, &bytes)?;

    // remove files a rename left behind
    for old_rel in &entry.files {
        if old_rel != &new_rel
            && let Ok(old_path) = crate::ledger::resolve_relative(old_rel)
            && old_path.is_file()
        {
            std::fs::remove_file(&old_path)?;
            changed = true;
        }
    }

    let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
    if !entry.config_refs.contains(&new_basename) {
        for r in &entry.config_refs {
            cfg.list_remove("extensions", r);
        }
        cfg.list_add("extensions", &new_basename);
        changed = true;
    }

    let mut led = Ledger::load()?;
    if let Some(e) = led.entries.iter_mut().find(|e| e.id == entry.id) {
        e.branch.clone_from(&item.branch);
        e.config_refs = vec![new_basename];
        e.files = vec![new_rel.clone()];
        e.resolved_urls = vec![url];
        e.hashes = std::collections::BTreeMap::from([(new_rel, new_hash)]);
    }
    led.save()?;
    cfg.save()?;
    Ok(changed)
}

/// Themes reinstall from scratch: wipe the folder, run the normal install
/// path (which rebuilds the ledger entry and the theme.js bridge), then
/// restore the user's colour scheme when it still exists upstream.
async fn update_theme(http: &HttpClient, entry: &LedgerEntry, item: &CardItem) -> Result<bool> {
    let folder = entry.config_refs.first().cloned().unwrap_or_default();

    // capture user state before the wipe
    let cfg_before = SpicetifyConfig::load(dirs::config_file())?;
    let active_scheme = if cfg_before.current_theme().as_deref() == Some(folder.as_str()) {
        cfg_before.color_scheme().filter(|s| !s.is_empty())
    } else {
        None
    };
    let old_hashes = entry.hashes.clone();
    let old_installed_at = entry.installed_at;

    let theme_root = dirs::themes_dir().join(&folder);

    // detect local drift (edited/extra files) before wiping, so the update
    // count reflects repairs even when upstream content is unchanged
    let mut changed = local_drift(&theme_root, entry);

    if theme_root.is_dir() {
        std::fs::remove_dir_all(&theme_root)?;
    }

    // the surviving ledger entry makes the installer reuse the same folder
    crate::commands::install::install_item(http, item, true).await?;

    // restore the user's colour scheme when it still exists upstream
    let available: Vec<String> = std::fs::read_to_string(theme_root.join("color.ini"))
        .map(|text| {
            crate::spicetify::schemes::parse_ini(&text)
                .keys()
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if let Some(scheme) =
        crate::spicetify::schemes::preferred_scheme(active_scheme.as_deref(), &available)
    {
        let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
        if cfg.color_scheme().as_deref() != Some(scheme.as_str()) {
            cfg.set_string("Setting", "color_scheme", &scheme);
            cfg.save()?;
            changed = true;
        }
    }

    // the installer rebuilt the entry; restore the install date and detect
    // real changes by comparing rebuilt vs previous file hashes
    let mut led = Ledger::load()?;
    if let Some(e) = led.entries.iter_mut().find(|e| e.id == entry.id) {
        if e.hashes != old_hashes {
            changed = true;
        }
        e.installed_at = old_installed_at;
    }
    led.save()?;
    Ok(changed)
}

/// True when the theme folder on disk no longer matches the ledger: files
/// edited, missing, or added outside a spicepm update.
fn local_drift(theme_root: &std::path::Path, entry: &LedgerEntry) -> bool {
    if !theme_root.is_dir() {
        return false; // nothing to drift away from
    }
    let mut seen = 0usize;
    let mut stack = vec![theme_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return true;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = crate::ledger::rel_from_base(&path);
            if !entry.files.contains(&rel) {
                return true; // extra file
            }
            match (std::fs::read(&path), entry.hashes.get(&rel)) {
                (Ok(bytes), Some(hash)) => {
                    if crate::ledger::hash_bytes(&bytes) != *hash {
                        return true; // edited file
                    }
                }
                _ => return true,
            }
            seen += 1;
        }
    }
    seen != entry.files.len() // tracked file missing from disk
}

/// The scheme to restore after a clean reinstall: the previous choice when
/// it still exists among the theme's colour schemes, otherwise nothing (the
/// installer's default stands).
async fn get_repo_manifests_fresh(
    http: &HttpClient,
    user: &str,
    repo: &str,
    branch: &str,
) -> Result<(Vec<crate::market::types::Manifest>, Vec<String>)> {
    let url = format!("https://raw.githubusercontent.com/{user}/{repo}/{branch}/manifest.json");
    let value = http.get_json_cached(&url, FRESH_TTL).await?;
    Ok(crate::market::validation::parse_manifests(
        &value,
        &format!("{user}/{repo}"),
    ))
}
