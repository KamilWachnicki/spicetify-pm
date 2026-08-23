use crate::cli::parse_target;
use crate::commands::load_blacklist;
use crate::errors::{Error, Result};
use crate::http::HttpClient;
use crate::market::items::{build_card_items, get_repo_manifests};
use crate::market::search::get_repo;
use crate::ui;
use serde::Serialize;

#[derive(Serialize)]
struct InfoOut {
    repo: String,
    url: String,
    stars: u64,
    archived: bool,
    default_branch: String,
    description: String,
    items: Vec<crate::market::types::CardItem>,
    warnings: Vec<String>,
}

pub async fn run(http: &HttpClient, target: &str, json: bool) -> Result<()> {
    let (user, repo_name) = parse_target(target)
        .ok_or_else(|| Error::other(format!("could not parse `{target}` as user/repo")))?;

    let spinner = ui::spinner(json, format!("fetching {user}/{repo_name}"));
    let repo = get_repo(http, &user, &repo_name).await?;
    let blacklist = load_blacklist(http).await?;
    if blacklist.is_blacklisted(&repo.html_url) {
        return Err(Error::other(format!(
            "{user}/{repo_name} is blacklisted by the official marketplace"
        )));
    }
    let (manifests, warnings) = get_repo_manifests(
        http,
        repo.full_name.split('/').next().unwrap_or(&user),
        &repo.name,
        &repo.default_branch,
    )
    .await?;
    let items = build_card_items(&repo, manifests);
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    if json {
        let out = InfoOut {
            repo: repo.full_name.clone(),
            url: repo.html_url.clone(),
            stars: repo.stargazers_count,
            archived: repo.archived,
            default_branch: repo.default_branch.clone(),
            description: repo.description.clone().unwrap_or_default(),
            warnings,
            items,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("{} {}", ui_style_repo(&repo.full_name), repo.html_url);
    println!("stars: {}", repo.stargazers_count);
    if repo.archived {
        ui::warn("this repository is archived");
    }
    if repo.description.as_deref().is_some_and(|d| !d.is_empty()) {
        println!("{}", repo.description.clone().unwrap_or_default());
    }
    println!();
    for warning in &warnings {
        ui::warn(warning);
    }
    if items.is_empty() {
        ui::info("no installable manifests (extensions/themes) in this repo");
        return Ok(());
    }
    for item in &items {
        println!(
            "{} {} - {}",
            match item.kind {
                crate::market::types::ItemKind::Extension => "[extension]",
                crate::market::types::ItemKind::Theme => "[theme]",
            },
            ui::style_title(&item.title),
            item.subtitle
        );
        println!("   id: {}", item.id());
        let authors = item
            .authors
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        println!("   authors: {authors}");
        println!("   branch: {}", item.branch);
        println!("   repo: {}", item.github_url());
        if !item.tags.is_empty() {
            println!("   tags: {}", item.tags.join(", "));
        }
        match item.kind {
            crate::market::types::ItemKind::Extension => {
                println!(
                    "   main: {}",
                    item.extension_url.clone().unwrap_or_default()
                );
            }
            crate::market::types::ItemKind::Theme => {
                println!("   usercss: {}", item.css_url.clone().unwrap_or_default());
                if let Some(schemes) = &item.schemes_url {
                    println!("   schemes: {schemes}");
                }
            }
        }
        println!();
    }
    Ok(())
}

fn ui_style_repo(name: &str) -> impl std::fmt::Display {
    console::style(name).bold()
}
