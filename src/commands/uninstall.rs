use crate::errors::{Error, Result};
use crate::http::HttpClient;
use crate::ledger::{Kind, Ledger};
use crate::spicetify::dirs;
use crate::spicetify::ini::SpicetifyConfig;
use crate::ui;

pub async fn run(http: &HttpClient, target: &str, yes: bool) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let mut led = Ledger::load()?;

    let matches = find_matches(&led, target);
    if matches.is_empty() {
        return Err(Error::other(format!(
            "no installed item matches `{target}`"
        )));
    }
    {
        // resolve which entry to remove (prompt when ambiguous)
        let (id, kind) = if matches.len() == 1 {
            let e = &led.entries[matches[0]];
            (e.id.clone(), e.kind)
        } else {
            let labels: Vec<String> = matches
                .iter()
                .map(|&i| {
                    let e = &led.entries[i];
                    format!("[{}] {}", format!("{:?}", e.kind).to_lowercase(), e.id)
                })
                .collect();
            if !console::Term::stderr().features().is_attended() {
                return Err(Error::other(format!(
                    "`{target}` is ambiguous: {}; be more specific",
                    labels.join(", ")
                )));
            }
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let idx = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
                .with_prompt("Multiple matches — which one?")
                .items(&refs)
                .default(0)
                .interact()
                .map_err(|e| Error::other(format!("selection cancelled: {e}")))?;
            let chosen = &led.entries[matches[idx]];
            (chosen.id.clone(), chosen.kind)
        };

        // safety gate unless --yes
        if !yes {
            let confirmed =
                dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
                    .with_prompt(format!(
                        "really uninstall {} {}?",
                        format!("{kind:?}").to_lowercase(),
                        ui::style_title(&id)
                    ))
                    .default(false)
                    .interact()
                    .unwrap_or(false);
            if !confirmed {
                ui::info("aborted — nothing was removed");
                return Ok(());
            }
        }

        remove_entry(&mut led, &id)?;
        if kind == Kind::Extension {
            crate::commands::reconcile_extensions_dir(&led)?;
            crate::commands::snippets::rebuild_companion(http).await?;
        }
        ui::success(format!(
            "uninstalled {} ({})",
            ui::style_title(&id),
            format!("{kind:?}").to_lowercase()
        ));
        ui::reminder_apply(crate::commands::apply_hook::requested());
        Ok(())
    }
}

/// Indices into `ledger.entries` matching a query: exact id, id fragment,
/// manifest name, or owned filename.
fn find_matches(led: &Ledger, target: &str) -> Vec<usize> {
    let t = target.to_lowercase();
    led.entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.id.to_lowercase() == t
                || e.id.to_lowercase().contains(&t)
                || e.files.iter().any(|f| {
                    f.rsplit('/')
                        .next()
                        .is_some_and(|b| b.eq_ignore_ascii_case(target))
                })
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn remove_entry(led: &mut Ledger, id: &str) -> Result<()> {
    let Some(entry) = led.remove(id) else {
        return Err(Error::other(format!("`{id}` not found in ledger")));
    };
    let mut cfg = SpicetifyConfig::load(dirs::config_file())?;

    match entry.kind {
        Kind::Theme => {
            // all theme files live under Themes/<folder>; remove that folder
            for folder in &entry.config_refs {
                let dir = dirs::themes_dir().join(folder);
                if dir.is_dir() {
                    std::fs::remove_dir_all(&dir)?;
                }
                // deactivate if it was active
                if cfg.current_theme().as_deref() == Some(folder.as_str()) {
                    cfg.set_string("Setting", "current_theme", "");
                    cfg.set_string("Setting", "color_scheme", "");
                }
            }
        }
        // extensions and the snippet companion both live in Extensions/
        // and are registered in the `extensions` config list
        Kind::Extension | Kind::SnippetGroup => {
            for file in &entry.files {
                if let Some(path) = safe_spice_path(file) {
                    let _ = std::fs::remove_file(path);
                }
            }
            for reference in &entry.config_refs {
                cfg.list_remove("extensions", reference);
            }
        }
    }

    crate::commands::persist_ledger(led)?;
    cfg.save()?;
    Ok(())
}

/// Resolve a ledger-relative path, ignoring entries outside the spicetify dir.
fn safe_spice_path(rel: &str) -> Option<std::path::PathBuf> {
    crate::ledger::resolve_relative(rel).ok()
}
