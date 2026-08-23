//! Snippet management + the spicepm companion extension: a tiny generated JS
//! file that injects all enabled snippets as a `<style>` tag - the same
//! mechanism the marketplace app uses at runtime, driven from our ledger.

use crate::errors::{Error, Result};
use crate::http::HttpClient;
use crate::ledger::{Kind, Ledger, LedgerEntry};
use crate::market::snippets::fetch_snippets;
use crate::market::types::Snippet;
use crate::spicetify::dirs;
use crate::spicetify::ini::SpicetifyConfig;
use crate::ui;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

pub const COMPANION_FILENAME: &str = "spicepm-snippets.js";

/// Reserved ledger identity for the snippet companion; the `@` prefix cannot
/// collide with any real `user/repo`.
pub const COMPANION_ID: &str = crate::ledger::SNIPPET_GROUP_ID;

#[derive(Serialize)]
struct SnippetRow {
    key: String,
    title: String,
    description: String,
    enabled: bool,
}

pub async fn run_search(http: &HttpClient, query: Option<&str>, json: bool) -> Result<()> {
    let mut snippets = fetch_snippets(http).await?;
    if let Some(q) = query.map(str::to_lowercase) {
        snippets.retain(|s| {
            s.key().to_lowercase().contains(&q)
                || s.title.to_lowercase().contains(&q)
                || s.description.to_lowercase().contains(&q)
        });
    }
    let enabled = enabled_keys(&Ledger::load()?);

    if json {
        let rows: Vec<SnippetRow> = snippets
            .iter()
            .map(|s| SnippetRow {
                key: s.key(),
                title: s.title.clone(),
                description: s.description.clone(),
                enabled: enabled.contains(&s.key()),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if snippets.is_empty() {
        match query {
            Some(q) => ui::info(format!("no snippets match `{q}`")),
            None => ui::info("no snippets available"),
        }
        return Ok(());
    }

    let term = console::Term::stdout();
    let interactive = term.features().is_attended();
    if !interactive {
        ui::info(format!("{} snippet(s)", snippets.len()));
        let enabled = enabled_keys(&Ledger::load()?);
        snippet_table(&snippets, &enabled, 0, snippets.len(), None).print();
        return Ok(());
    }

    // interactive keyboard paging (TTY only); digits toggle a snippet and
    // the pager stays open so several can be flipped in one session
    let mut any_toggled = false;
    let mut current_page = 0usize;
    loop {
        // fresh per frame: toggles from this session show up immediately
        let enabled = enabled_keys(&Ledger::load()?);
        let render = |p: &ui::PagerPage| -> Vec<String> {
            vec![ui::info_line(format!(
                "{} snippet(s) - page {}/{} (showing {}–{})",
                snippets.len(),
                p.page + 1,
                p.pages,
                p.start + 1,
                p.end
            ))]
            .into_iter()
            .chain(snippet_table(&snippets, &enabled, p.start, p.end, p.width).to_lines())
            .collect()
        };

        match ui::run_pager(
            &term,
            snippets.len(),
            &ui::pager_footer("toggle"),
            render,
            &mut current_page,
        )? {
            ui::PagerExit::Quit => break,
            ui::PagerExit::Selected(idx) => {
                dirs::require_spicetify_dir()?;
                let snippet = &snippets[idx];
                let key = snippet.key();
                let was_enabled = enabled.contains(&key);
                let mut led = Ledger::load()?;
                let changed = if was_enabled {
                    disable_key(&mut led, &snippets, &key)?
                } else {
                    enable_key(&mut led, &snippets, &key)?
                };
                led.save()?;
                if changed {
                    any_toggled = true;
                    if was_enabled {
                        ui::success(format!(
                            "disabled snippet {}",
                            ui::style_title(&snippet.title)
                        ));
                    } else {
                        ui::success(format!(
                            "enabled snippet {}",
                            ui::style_title(&snippet.title)
                        ));
                    }
                } else {
                    ui::info("no change");
                }
            }
        }
    }

    if any_toggled {
        println!();
        ui::reminder_apply(crate::commands::apply_hook::requested());
    }
    Ok(())
}

const DESC_WIDTH: usize = 60;

fn snippet_table(
    all: &[Snippet],
    enabled: &[String],
    start: usize,
    end: usize,
    term_width: Option<usize>,
) -> ui::Table {
    const SEPARATORS: usize = 6; // 4 columns -> 3 double-space gaps
    let desc_budget = match term_width {
        None => DESC_WIDTH,
        Some(width) => {
            let widest_fixed = all[start..end]
                .iter()
                .map(|s| {
                    let num = "#".len();
                    let key = s.key().chars().count().max("KEY".len());
                    num + key + "ENABLED".len()
                })
                .max()
                .unwrap_or(0)
                + "DESCRIPTION".len()
                + SEPARATORS;
            width.saturating_sub(widest_fixed).clamp(10, DESC_WIDTH)
        }
    };

    let mut table = ui::Table::new(&["#", "KEY", "ENABLED", "DESCRIPTION"]);
    for (offset, s) in all[start..end].iter().enumerate() {
        let is_enabled = enabled.contains(&s.key());
        let key_cell = if is_enabled {
            console::style(format!("\u{2714} {}", truncate_key(&s.key(), 40)))
                .green()
                .bold()
                .to_string()
        } else {
            truncate_key(&s.key(), 40)
        };
        let state_cell = if is_enabled {
            console::style("yes").green().bold().to_string()
        } else {
            String::new()
        };
        table.row(vec![
            offset.to_string(),
            key_cell,
            state_cell,
            crate::commands::search::truncate(&s.description, desc_budget),
        ]);
    }
    table
}

fn truncate_key(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_owned()
    } else {
        format!("{}\u{2026}", s.chars().take(width - 1).collect::<String>())
    }
}

pub async fn run_show(http: &HttpClient, name: &str) -> Result<()> {
    let snippets = fetch_snippets(http).await?;
    let snippet = find_snippet(&snippets, name)?;
    println!("/* {} - {} */", snippet.title, snippet.description);
    println!("{}", snippet.code);
    Ok(())
}

/// Enable a snippet and regenerate the companion extension.
pub async fn run_add(http: &HttpClient, name: &str) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let snippets = fetch_snippets(http).await?;
    let snippet = find_snippet(&snippets, name)?;

    let mut led = Ledger::load()?;
    enable_key(&mut led, &snippets, &snippet.key())?;
    led.save()?;

    ui::success(format!(
        "enabled snippet {}",
        ui::style_title(&snippet.title)
    ));
    ui::reminder_apply(crate::commands::apply_hook::requested());
    Ok(())
}

/// Enable a snippet by key and regenerate the companion extension.
/// Returns false when it was already enabled. Used by `snippets add`
/// and the pager's digit selection in `snippets search`.
pub(crate) fn enable_key(led: &mut Ledger, all: &[Snippet], key: &str) -> Result<bool> {
    let mut keys = enabled_keys(led);
    if keys.iter().any(|k| k == key) {
        return Ok(false);
    }
    keys.push(key.to_owned());
    let enabled = resolve_enabled(all, &keys);
    write_companion(led, &enabled, keys)?;
    Ok(true)
}

pub async fn run_remove(http: &HttpClient, name: &str) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let snippets = fetch_snippets(http).await?;
    let snippet = find_snippet(&snippets, name)?;

    let mut led = Ledger::load()?;
    if !disable_key(&mut led, &snippets, &snippet.key())? {
        ui::info(format!("snippet `{}` was not enabled", snippet.key()));
        return Ok(());
    }
    led.save()?;

    ui::success(format!(
        "disabled snippet {}",
        ui::style_title(&snippet.title)
    ));
    ui::reminder_apply(crate::commands::apply_hook::requested());
    Ok(())
}

/// Disable a snippet by key and regenerate the companion extension.
/// Returns false when it was not enabled. Used by `snippets remove`.
pub(crate) fn disable_key(led: &mut Ledger, all: &[Snippet], key: &str) -> Result<bool> {
    let keys: Vec<String> = enabled_keys(led).into_iter().filter(|k| k != key).collect();
    if keys == enabled_keys(led) {
        return Ok(false);
    }
    let enabled = resolve_enabled(all, &keys);
    write_companion(led, &enabled, keys)?;
    Ok(true)
}

/// Regenerate the companion from the ledger's stored keys (used by update).
/// Regenerate the companion extension purely from the current ledger state,
/// fetching snippet bodies as needed. Used after extension installs and
/// removals so the auto-generated file is always rebuilt from scratch.
pub(crate) async fn rebuild_companion(http: &HttpClient) -> Result<()> {
    let mut led = Ledger::load()?;
    let keys = enabled_keys(&led);
    let had_file = dirs::extensions_dir().join(COMPANION_FILENAME).is_file();
    if keys.is_empty() && !had_file {
        return Ok(()); // nothing enabled, nothing to clean up
    }
    if keys.is_empty() {
        // stale companion with no enabled snippets: remove the file and its
        // config registration
        let dest = dirs::extensions_dir().join(COMPANION_FILENAME);
        if dest.is_file() {
            std::fs::remove_file(&dest)?;
        }
        let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
        cfg.list_remove("extensions", COMPANION_FILENAME);
        cfg.save()?;
        led.remove(COMPANION_ID);
        crate::commands::persist_ledger(&mut led)?;
        return Ok(());
    }

    let all = fetch_snippets(http).await?;
    let enabled = resolve_enabled(&all, &keys);
    write_companion(&mut led, &enabled, keys)?;
    crate::commands::persist_ledger(&mut led)?;
    Ok(())
}

pub async fn regenerate_companion(http: &HttpClient, entry: &LedgerEntry) -> Result<bool> {
    let keys = entry.snippet_keys.clone();
    let before_hash = entry.hashes.values().next().cloned().unwrap_or_default();

    let all = fetch_snippets(http).await?;
    let enabled = resolve_enabled(&all, &keys);
    let mut led = Ledger::load()?;
    write_companion(&mut led, &enabled, keys)?;
    led.save()?;

    let after_hash = led
        .find(COMPANION_ID)
        .and_then(|e| e.hashes.values().next().cloned())
        .unwrap_or_default();
    Ok(before_hash != after_hash)
}

/// (Re)write the companion extension and its ledger/config entries.
fn write_companion(led: &mut Ledger, enabled: &[Snippet], keys: Vec<String>) -> Result<()> {
    let js = generate_js(enabled);
    let ext_dir = dirs::extensions_dir();
    std::fs::create_dir_all(&ext_dir)?;
    let dest = ext_dir.join(COMPANION_FILENAME);
    let bytes = js.as_bytes();
    crate::cache::atomic_write(&dest, bytes)?;

    let rel = crate::ledger::rel_from_base(&dest);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let existing = led.find(COMPANION_ID).cloned();
    led.upsert(LedgerEntry {
        id: COMPANION_ID.to_owned(),
        kind: Kind::SnippetGroup,
        user: "spicepm".to_owned(),
        repo: "snippets".to_owned(),
        branch: String::new(),
        files: vec![rel.clone()],
        config_refs: vec![COMPANION_FILENAME.to_owned()],
        resolved_urls: vec![crate::market::constants::SNIPPETS_URL.to_owned()],
        hashes: [(rel, crate::ledger::hash_bytes(bytes))]
            .into_iter()
            .collect(),
        snippet_keys: keys,
        installed_at: existing.map_or(now, |e| e.installed_at),
    });

    // register the companion like any other extension
    let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
    cfg.list_add("extensions", COMPANION_FILENAME);
    cfg.save()?;
    Ok(())
}

pub(crate) fn enabled_keys(led: &Ledger) -> Vec<String> {
    led.find(COMPANION_ID)
        .map(|e| e.snippet_keys.clone())
        .unwrap_or_default()
}

fn resolve_enabled(all: &[Snippet], keys: &[String]) -> Vec<Snippet> {
    all.iter()
        .filter(|s| keys.iter().any(|k| k == &s.key()))
        .cloned()
        .collect()
}

fn find_snippet<'a>(snippets: &'a [Snippet], name: &str) -> Result<&'a Snippet> {
    let lower = name.to_lowercase();
    snippets
        .iter()
        .find(|s| s.key().to_lowercase() == lower || s.title.to_lowercase() == lower)
        .ok_or_else(|| {
            Error::other(format!(
                "no snippet named `{name}`; run `spice-pm snippets search`"
            ))
        })
}

/// Generate the companion extension JS with all enabled snippet CSS embedded,
/// mirroring the marketplace's `initializeSnippets` output.
pub fn generate_js(enabled: &[Snippet]) -> String {
    use std::fmt::Write as _;
    let css: String = enabled.iter().fold(String::new(), |mut acc, s| {
        let _ = write!(acc, "/* {} - {} */\n{}\n", s.title, s.description, s.code);
        acc
    });
    format!(
        "// Generated by spice-pm {version} - do not edit by hand.\n\
         // Re-run `spice-pm snippets add|remove` to change this file.\n\
         (function () {{\n\
         \x20 \"use strict\";\n\
         \x20 function inject() {{\n\
         \x20   var existing = document.querySelector(\"style.spicepmSnippets\");\n\
         \x20   if (existing) existing.remove();\n\
         \x20   var css = `{css}`;\n\
         \x20   var style = document.createElement(\"style\");\n\
         \x20   style.textContent = css;\n\
         \x20   style.className = \"spicepmSnippets\";\n\
         \x20   document.body.appendChild(style);\n\
         \x20 }}\n\
         \x20 if (document.body) {{\n\
         \x20   inject();\n\
         \x20 }} else {{\n\
         \x20   window.addEventListener(\"DOMContentLoaded\", inject);\n\
         \x20 }}\n\
         }})();\n",
        version = env!("CARGO_PKG_VERSION"),
        css = escape_js_template(&css),
    )
}

/// Escape a string for embedding inside a JS template literal.
fn escape_js_template(css: &str) -> String {
    css.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(title: &str, code: &str) -> Snippet {
        Snippet {
            title: title.to_owned(),
            description: "desc".to_owned(),
            code: code.to_owned(),
            preview: None,
            image_url: None,
        }
    }

    #[test]
    fn companion_contains_css_and_escapes() {
        let js = generate_js(&[
            snip("Backtick `Test", "a { color: red }"),
            snip("Dollar ${Sign", "b { color: blue }"),
        ]);
        assert!(js.contains("a { color: red }"));
        assert!(js.contains("b { color: blue }"));
        assert!(js.contains("\\`Test"));
        assert!(js.contains("\\${Sign"));
        assert!(js.contains("spicepmSnippets"));
    }

    #[test]
    fn companion_empty_is_valid() {
        let js = generate_js(&[]);
        assert!(js.contains("var css = ``;"));
    }
}
