use std::cmp::Reverse;

use crate::cli::{ItemTypeArg, SortArg};
use crate::commands::{load_blacklist, topic_from_type};
use crate::errors::Result;
use crate::http::HttpClient;
use crate::market::constants::{ITEMS_PER_REQUEST, RepoTopic};
use crate::market::items::expand_repos_to_items;
use crate::market::search::{SearchOptions, get_tagged_repos};
use crate::market::types::{CardItem, ItemKind, Repo};
use crate::ui;
use serde::Serialize;

const DESC_WIDTH: usize = 60;
/// Safety bound on unauthenticated pagination (search API: 10 req/min).
const MAX_SEARCH_PAGES: u32 = 10;

/// One display row = one *manifest item* (not a repo), matching how the
/// marketplace grid renders cards.
struct Row {
    repo: Repo,
    item: CardItem,
}

#[derive(Serialize)]
struct SearchRow {
    id: String,
    title: String,
    kind: &'static str,
    user: String,
    repo: String,
    branch: String,
    stars: u64,
    tags: Vec<String>,
    description: String,
    url: String,
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    http: &HttpClient,
    query: Option<String>,
    r#type: Option<ItemTypeArg>,
    sort: SortArg,
    page: Option<u32>,
    archived: bool,
    json: bool,
) -> Result<()> {
    let topic_vec: Vec<RepoTopic> = match topic_from_type(r#type) {
        Some(t) => vec![t],
        None => vec![RepoTopic::Extensions, RepoTopic::Themes],
    };
    let blacklist = load_blacklist(http).await?;

    let mut repos: Vec<Repo> = Vec::new();
    let mut total_on_github = 0u64;
    let mut filtered_by_rules = 0usize;
    // GitHub's topic-search ordering jitters between calls, so a single page
    // can miss repos; fetch everything unless the user asked for one page
    for &topic in &topic_vec {
        let scope = match page {
            Some(p) => format!("page {p}"),
            None => "all pages".to_owned(),
        };
        let spinner = ui::spinner(
            json,
            format!("searching topic:{topic} ({scope})", topic = topic.as_str()),
        );

        let mut pageno = page.unwrap_or(1);
        loop {
            let result = get_tagged_repos(
                http,
                &blacklist,
                SearchOptions {
                    topic,
                    page: pageno,
                    include_archived: archived,
                },
            )
            .await?;
            total_on_github += if pageno == 1 { result.total_count } else { 0 };
            filtered_by_rules += result.blacklisted_filtered + result.archived_filtered;
            let fetched = result.items.len();
            repos.extend(result.items);
            let single_page_requested = page.is_some();
            if single_page_requested
                || fetched < ITEMS_PER_REQUEST as usize
                || pageno >= MAX_SEARCH_PAGES
            {
                break;
            }
            pageno += 1;
        }
        if let Some(pb) = spinner {
            pb.finish_and_clear();
        }
    }

    // a repo can be tagged for more than one topic; expand it once
    repos.sort_by(|a, b| a.full_name.cmp(&b.full_name));
    repos.dedup_by(|a, b| a.full_name == b.full_name);

    // fetch + validate every repo's manifest.json, expanding to items;
    // repos without any valid manifest are dropped entirely
    let spinner = ui::spinner(
        json,
        format!("validating manifests for {} repos", repos.len()),
    );
    let (expanded, invalid_manifests) = expand_repos_to_items(http, &repos).await;
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }
    let valid_repos = expanded.len();

    let mut rows: Vec<Row> = expanded
        .into_iter()
        .flat_map(|(repo, items)| {
            items.into_iter().map(move |item| Row {
                repo: repo.clone(),
                item,
            })
        })
        .collect();

    if let Some(q) = query.as_deref() {
        rows.retain(|row| matches_item(&row.item, &row.repo, q));
    }

    // sort modes mirror the marketplace's sortCardItems; date keys come from
    // the owning repo (manifests don't carry dates)
    match sort {
        SortArg::Stars => rows.sort_by_key(|row| Reverse(row.repo.stargazers_count)),
        SortArg::Newest => rows.sort_by_key(|row| Reverse(row.repo.created_at.clone())),
        SortArg::Oldest => rows.sort_by_key(|row| row.repo.created_at.clone()),
        SortArg::LastUpdated => rows.sort_by_key(|row| Reverse(row.repo.updated_at.clone())),
        SortArg::MostStale => rows.sort_by_key(|row| row.repo.updated_at.clone()),
        SortArg::Az => rows.sort_by_key(|row| row.item.title.to_lowercase()),
        SortArg::Za => rows.sort_by_key(|row| Reverse(row.item.title.to_lowercase())),
    }

    if json {
        let items: Vec<SearchRow> = rows.iter().map(SearchRow::from_row).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "page": page,
                "per_page": ITEMS_PER_REQUEST,
                "count": items.len(),
                "repos_expanded": valid_repos,
                "hidden_by_marketplace_rules": filtered_by_rules,
                "invalid_manifests_skipped": invalid_manifests,
                "items": items,
            }))?
        );
        return Ok(());
    }

    if rows.is_empty() {
        ui::info("no results");
        return Ok(());
    }
    let scope = match page {
        Some(p) => format!("page {p}"),
        None => "all pages".to_owned(),
    };
    let summary = format!(
        "{} item(s) from {valid_repos} repo(s) ({scope}) \
         ({total_on_github} repos on GitHub, {filtered_by_rules} hidden by marketplace rules, \
         {invalid_manifests} invalid manifests skipped)",
        rows.len(),
    );
    let install_hint = format!(
        "install with {} (append {} to pick one of several manifests in the same repo)",
        ui::style_title("spice-pm install <user/repo[#title]>"),
        "#Title"
    );

    let term = console::Term::stdout();
    let interactive = term.features().is_attended();

    if !interactive {
        ui::info(summary);
        let installed = crate::ledger::Ledger::installed_ids();
        let table = result_table(&rows, 0, rows.len(), None, &installed);
        table.print();
        println!();
        ui::info(install_hint);
        return Ok(());
    }

    // interactive keyboard paging (TTY only); the shared pager repaints one
    // frame in place so key-repeat can't stack a wall of copies
    let render = |p: &ui::PagerPage| -> Vec<String> {
        // fresh per frame: installs from this session (or another terminal)
        // show up immediately
        let installed = crate::ledger::Ledger::installed_ids();
        vec![ui::info_line(format!(
            "{summary} - page {}/{} (showing {}–{})",
            p.page + 1,
            p.pages,
            p.start + 1,
            p.end
        ))]
        .into_iter()
        .chain(result_table(&rows, p.start, p.end, p.width, &installed).to_lines())
        .collect()
    };

    // pager stays open so several items can be handled per session:
    // digits install uninstalled extensions/themes and toggle (remove)
    // extensions that are already in
    let mut changes_count = 0usize;
    let mut current_page = 0usize;
    loop {
        match ui::run_pager(
            &term,
            rows.len(),
            &ui::pager_footer("install"),
            render,
            &mut current_page,
        )? {
            ui::PagerExit::Selected(idx) => {
                let item = &rows[idx].item;
                // authoritative at decision time: earlier toggles in this
                // session already changed the ledger
                let was_installed = crate::ledger::Ledger::installed_ids().contains(&item.id());

                match (item.kind, was_installed) {
                    // already in: digit toggles to remove (confirmed)
                    (crate::market::types::ItemKind::Extension, true) => {
                        print_remove_summary(item);
                        let confirmed = dialoguer::Confirm::with_theme(
                            &dialoguer::theme::ColorfulTheme::default(),
                        )
                        .with_prompt("really remove this extension?")
                        .default(false)
                        .interact()
                        .unwrap_or(false);
                        if !confirmed {
                            ui::info("skipped - still installed");
                            continue;
                        }
                        let mut led = crate::ledger::Ledger::load()?;
                        crate::commands::remove::remove_entry(&mut led, &item.id())?;
                        ui::success(format!(
                            "removed extension {}",
                            ui::style_title(&item.title)
                        ));
                    }

                    // installed: digit removes the theme entirely while
                    // staying in the pager. Config is only cleared when
                    // this exact theme is the one currently applied.
                    (crate::market::types::ItemKind::Theme, true) => {
                        print_remove_summary(item);
                        let confirmed = dialoguer::Confirm::with_theme(
                            &dialoguer::theme::ColorfulTheme::default(),
                        )
                        .with_prompt("really remove this theme?")
                        .default(false)
                        .interact()
                        .unwrap_or(false);
                        if !confirmed {
                            ui::info("skipped - still installed");
                            continue;
                        }
                        let mut led = crate::ledger::Ledger::load()?;
                        crate::commands::remove::remove_entry(&mut led, &item.id())?;
                        ui::success(format!(
                            "removed theme {}",
                            ui::style_title(&item.title)
                        ));
                    }

                    // not installed: themes are heavy (folder rewrite +
                    // scheme choice), so confirm before one goes in;
                    // declining keeps browsing
                    (crate::market::types::ItemKind::Theme, false) => {
                        print_install_summary(item);
                        let confirmed = dialoguer::Confirm::with_theme(
                            &dialoguer::theme::ColorfulTheme::default(),
                        )
                        .with_prompt("proceed with install?")
                        .default(false)
                        .interact()
                        .unwrap_or(false);
                        if !confirmed {
                            ui::info(format!("skipped {}", item.id()));
                            continue;
                        }

                        println!();
                        ui::info(format!(
                            "installing {} [{}] ...",
                            ui::style_title(&item.title),
                            item.id()
                        ));
                        // keep prompts available (e.g. colour-scheme choice)
                        crate::commands::install::install_item(http, item, false).await?;

                        // one theme operation per session
                        println!();
                        ui::reminder_apply(crate::commands::apply_hook::requested());
                        break;
                    }

                    // fresh install: summary then confirm (accidental
                    // keypresses shouldn't touch the system)
                    (crate::market::types::ItemKind::Extension, false) => {
                        print_install_summary(item);
                        let confirmed = dialoguer::Confirm::with_theme(
                            &dialoguer::theme::ColorfulTheme::default(),
                        )
                        .with_prompt("proceed with install?")
                        .default(false)
                        .interact()
                        .unwrap_or(false);
                        if !confirmed {
                            ui::info(format!("skipped {}", item.id()));
                            continue;
                        }
                        println!();
                        ui::info(format!(
                            "installing {} [{}] ...",
                            ui::style_title(&item.title),
                            item.id()
                        ));
                        crate::commands::install::install_item(http, item, false).await?;
                    }
                }
                changes_count += 1;
            }
            ui::PagerExit::Quit => {
                println!();
                if changes_count > 0 {
                    ui::reminder_apply(crate::commands::apply_hook::requested());
                } else {
                    ui::info(install_hint);
                }
                break;
            }
        }
    }
    Ok(())
}
/// Print everything a selection will install: item identity, source, and
/// the exact files/destinations on disk.
fn print_install_summary(item: &crate::market::types::CardItem) {
    let authors = item
        .authors
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    ui::info(format!(
        "about to install {} [{}] by {}",
        ui::style_title(&item.title),
        item.id(),
        authors
    ));
    println!("         from {} ({})", item.github_url(), item.branch);
    match item.kind {
        crate::market::types::ItemKind::Extension => {
            let basename = crate::commands::install::file_name_from(
                item.manifest.main.as_deref().unwrap_or_default(),
            );
            println!("         into Extensions/{basename}");
        }
        crate::market::types::ItemKind::Theme => {
            let folder = crate::commands::install::sanitize_folder_name(&item.manifest.name);
            let mut files = vec!["user.css".to_owned()];
            if item.schemes_url.is_some() {
                files.push("color.ini".to_owned());
            }
            for inc in &item.manifest.include {
                files.push(
                    crate::commands::install::include_dest_rel(inc).unwrap_or_else(|_| inc.clone()),
                );
            }
            if item
                .manifest
                .include
                .iter()
                .any(|i| i.to_lowercase().ends_with(".js"))
            {
                files.push("theme.js (auto-injected)".to_owned());
            }
            println!("         into Themes/{folder}/:");
            for f in &files {
                println!("           - {f}");
            }
        }
    }
}

/// Print everything a selection will remove from disk and config when the
/// digit toggles an installed extension to removed.
fn print_remove_summary(item: &crate::market::types::CardItem) {
    ui::info(format!(
        "about to remove {} [{}]",
        ui::style_title(&item.title),
        item.id()
    ));
    match item.kind {
        crate::market::types::ItemKind::Extension => {
            let basename = crate::commands::install::file_name_from(
                item.manifest.main.as_deref().unwrap_or_default(),
            );
            println!("         delete Extensions/{basename}");
            println!("         clear config extensions entry '{basename}'");
        }
        crate::market::types::ItemKind::Theme => {
            let folder = crate::commands::install::sanitize_folder_name(&item.manifest.name);
            println!("         delete Themes/{folder}/ (all files)");
            println!("         reset current_theme and color_scheme");
        }
    }
}

fn result_table(
    rows: &[Row],
    start: usize,
    end: usize,
    term_width: Option<usize>,
    installed: &std::collections::HashSet<String>,
) -> ui::Table {
    let mut table = ui::Table::new(&["#", "TITLE", "TYPE", "STATUS", "STARS", "DESCRIPTION"]);
    let desc_budget = desc_width(rows, term_width);
    for (offset, row) in rows[start..end].iter().enumerate() {
        let status = if installed.contains(&row.item.id()) {
            console::style("\u{2714} installed")
                .green()
                .bold()
                .to_string()
        } else {
            String::new()
        };
        table.row(vec![
            offset.to_string(),
            truncate(&row.item.title, 32),
            kind_label(row.item.kind).to_owned(),
            status,
            row.repo.stargazers_count.to_string(),
            truncate(&row.item.subtitle, desc_budget),
        ]);
    }
    table
}

/// Shrink the description column so the widest possible row still fits on
/// one terminal line (wrapped rows would break in-place repaints).
/// Computed over *all* rows so column widths stay stable while paging.
fn desc_width(rows: &[Row], term_width: Option<usize>) -> usize {
    const SEPARATORS: usize = 10; // 6 columns -> 5 double-space gaps
    // STATUS shows "✔ installed" at most; its visible width is constant
    const STATUS_MAX: usize = 11;
    let Some(width) = term_width else {
        return DESC_WIDTH;
    };
    let widest_fixed = rows
        .iter()
        .map(|row| {
            let title = truncate(&row.item.title, 32).chars().count();
            let kind = kind_label(row.item.kind).len();
            let stars = row.repo.stargazers_count.to_string().len();
            "#".len()
                + title.max("TITLE".len())
                + kind.max("TYPE".len())
                + STATUS_MAX.max("STATUS".len())
                + stars.max("STARS".len())
        })
        .max()
        .unwrap_or(0);
    let fixed = widest_fixed + "DESCRIPTION".len() + SEPARATORS;
    width.saturating_sub(fixed).clamp(10, DESC_WIDTH)
}

impl SearchRow {
    fn from_row(row: &Row) -> Self {
        Self {
            id: row.item.id(),
            title: row.item.title.clone(),
            kind: kind_label(row.item.kind),
            user: row.item.user.clone(),
            repo: row.item.repo.clone(),
            branch: row.item.branch.clone(),
            stars: row.repo.stargazers_count,
            tags: row.item.tags.clone(),
            description: row.item.subtitle.clone(),
            url: row.item.github_url(),
        }
    }
}

fn kind_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Extension => "extension",
        ItemKind::Theme => "theme",
    }
}

/// Query match across manifest title/description/tags/authors and repo
/// name/full-name/description.
fn matches_item(item: &CardItem, repo: &Repo, query: &str) -> bool {
    let q = query.to_lowercase();
    let mut haystack = String::with_capacity(128);
    haystack.push_str(&item.title);
    haystack.push(' ');
    haystack.push_str(&item.subtitle);
    haystack.push(' ');
    haystack.push_str(&item.user);
    haystack.push(' ');
    haystack.push_str(&item.repo);
    haystack.push(' ');
    haystack.push_str(&repo.full_name);
    haystack.push(' ');
    if let Some(desc) = &repo.description {
        haystack.push_str(desc);
        haystack.push(' ');
    }
    for tag in &item.tags {
        haystack.push_str(tag);
        haystack.push(' ');
    }
    for author in &item.authors {
        haystack.push_str(&author.name);
        haystack.push(' ');
    }
    haystack.to_lowercase().contains(&q)
}

pub(crate) fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(width.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_descriptions() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(
            truncate("a very long description here", 10),
            "a very lo\u{2026}"
        );
        assert_eq!(truncate("exact-ten!", 10), "exact-ten!");
    }

    #[test]
    fn table_has_numbered_uppercase_columns() {
        let repo: Repo = serde_json::from_str(
            r#"{
                "name": "themes",
                "full_name": "u/themes",
                "html_url": "https://github.com/u/themes",
                "stargazers_count": 7,
                "default_branch": "main"
            }"#,
        )
        .unwrap();
        let items = crate::market::items::build_card_items(
            &repo,
            crate::market::validation::parse_manifests(
                &serde_json::json!([
                    { "name": "A", "description": "da", "usercss": "a.css" },
                    { "name": "B", "description": "db", "usercss": "b.css" }
                ]),
                "u/themes",
            )
            .0,
        );
        let rows: Vec<Row> = items
            .into_iter()
            .map(|item| Row {
                repo: repo.clone(),
                item,
            })
            .collect();
        let installed: std::collections::HashSet<String> =
            [rows[0].item.id()].into_iter().collect();
        let lines = result_table(&rows, 0, rows.len(), None, &installed).to_lines();
        let header = &lines[0];
        for expected in ["#", "TITLE", "TYPE", "STATUS", "STARS", "DESCRIPTION"] {
            assert!(
                header.contains(expected),
                "header missing `{expected}`: {header}"
            );
        }
        assert!(!header.contains("kind"));
        assert!(!header.contains("USER/REPO"));
        // page-local numbering starts at 0
        assert!(lines[1].starts_with('0'));
        assert!(lines[2].starts_with('1'));
        // row 0 is installed: status marker present (colors are stripped
        // without a TTY; alignment is covered by the visible_len test)
        assert!(
            lines[1].contains("installed"),
            "status missing: {}",
            lines[1]
        );
        assert!(
            !lines[2].contains("installed"),
            "not-installed row must have empty status"
        );
    }

    #[test]
    fn matches_across_manifest_fields() {
        let repo: Repo = serde_json::from_str(
            r#"{
                "name": "spicetify-extensions",
                "full_name": "rxri/spicetify-extensions",
                "html_url": "https://github.com/rxri/spicetify-extensions",
                "stargazers_count": 1,
                "default_branch": "main"
            }"#,
        )
        .unwrap();
        let manifests = crate::market::validation::parse_manifests(
            &serde_json::json!([
                { "name": "fullAppDisplay", "description": "fullscreen display", "main": "fad.js" },
                { "name": "historyShortcut", "description": "hotkey", "main": "hs.js" }
            ]),
            "rxri/spicetify-extensions",
        )
        .0;
        let items = crate::market::items::build_card_items(&repo, manifests);
        assert_eq!(items.len(), 2, "multi-manifest repo expands to two items");

        assert!(matches_item(&items[0], &repo, "fullscreen"));
        assert!(matches_item(&items[0], &repo, "rxri"));
        assert!(!matches_item(&items[0], &repo, "hotkey"));
        assert!(matches_item(&items[1], &repo, "hotkey"));
    }

    #[test]
    fn query_match_is_case_insensitive_on_both_sides() {
        // regression: the haystack must be lowercased too, or "Dribbblish"
        // never matched a lowercase query
        let repo: Repo = serde_json::from_str(
            r#"{
                "name": "spicetify-themes",
                "full_name": "spicetify/spicetify-themes",
                "html_url": "https://github.com/spicetify/spicetify-themes",
                "stargazers_count": 1,
                "default_branch": "master"
            }"#,
        )
        .unwrap();
        let item = crate::market::items::build_card_items(
            &repo,
            crate::market::validation::parse_manifests(
                &serde_json::json!([
                    { "name": "Dribbblish", "description": "Dribbblish", "usercss": "Dribbblish/user.css" }
                ]),
                "spicetify/spicetify-themes",
            )
            .0,
        )
        .remove(0);

        assert!(matches_item(&item, &repo, "dribbblish"));
        assert!(matches_item(&item, &repo, "DRIBBBlish"));
        assert!(matches_item(&item, &repo, "Spicetify/Spicetify-Themes"));
    }
}
