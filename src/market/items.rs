//! Building installable items from repo manifests.
//! Ports of `fetchExtensionManifest` / `fetchThemeManifest`.

use super::constants::MANIFEST_TTL_SECS;
use super::types::{Author, CardItem, ItemKind, Manifest, Repo};
use super::urls::resolve_or_raw;
use crate::errors::Result;
use crate::http::HttpClient;
use futures::StreamExt;

const MAX_CONCURRENT_MANIFESTS: usize = 10;

/// Fetch and validate `manifest.json` for a repo (single object or array),
/// mirroring `getRepoManifest`. Returns parsed manifests plus warnings for
/// invalid entries.
pub async fn get_repo_manifests(
    http: &HttpClient,
    user: &str,
    repo: &str,
    branch: &str,
) -> Result<(Vec<Manifest>, Vec<String>)> {
    let url = format!("https://raw.githubusercontent.com/{user}/{repo}/{branch}/manifest.json");
    let value = http.get_json_cached(&url, MANIFEST_TTL_SECS).await?;
    let label = format!("{user}/{repo}");
    Ok(super::validation::parse_manifests(&value, &label))
}

/// Fetch and validate manifests for many repos concurrently and expand each
/// repo into its individual `CardItem`s - mirroring the marketplace grid,
/// where a multi-manifest repo (e.g. `rxri/spicetify-extensions`) surfaces as
/// one card per manifest entry. Repos with no valid manifests are dropped.
/// Returns per-repo item lists plus the count of malformed manifest warnings.
pub async fn expand_repos_to_items(
    http: &HttpClient,
    repos: &[Repo],
) -> (Vec<(Repo, Vec<CardItem>)>, usize) {
    type FetchedManifests = Result<(Vec<Manifest>, Vec<String>)>;
    let results: Vec<(Repo, FetchedManifests)> = futures::stream::iter(repos.iter().cloned())
        .map(|repo| async move {
            let owner = repo
                .full_name
                .split('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            let fetched = get_repo_manifests(http, &owner, &repo.name, &repo.default_branch).await;
            (repo, fetched)
        })
        .buffer_unordered(MAX_CONCURRENT_MANIFESTS)
        .collect()
        .await;

    let mut out = Vec::with_capacity(results.len());
    let mut warnings = 0usize;
    for (repo, fetched) in results {
        if let Ok((manifests, repo_warnings)) = fetched {
            warnings += repo_warnings.len();
            let items = build_card_items(&repo, manifests);
            // repos whose manifests are all invalid/absent are hidden,
            // exactly like the web app renders nothing for them
            if !items.is_empty() {
                out.push((repo, items));
            }
        } // missing manifest.json or transient fetch error -> no cards
    }
    (out, warnings)
}

/// Build all `CardItem`s for a repo. Each manifest is classified by kind;
/// manifests that are neither extension nor theme are skipped (custom apps
/// arrive in a later milestone).
///
/// User/repo extraction prefers the API's `contents_url` exactly like the
/// web app does (`contents_url` regex in FetchRemotes.ts).
pub fn build_card_items(repo: &Repo, manifests: Vec<Manifest>) -> Vec<CardItem> {
    let (contents_user, _) =
        crate::market::urls::parse_contents_url(repo.contents_url.as_deref().unwrap_or_default())
            .unwrap_or_else(|| {
                (
                    repo.full_name
                        .split('/')
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    String::new(),
                )
            });

    let mut items = Vec::new();
    for manifest in manifests {
        let Some(kind) = ItemKind::from_manifest(&manifest) else {
            continue;
        };
        let branch = manifest.selected_branch(&repo.default_branch).to_owned();
        let user = if contents_user.is_empty() {
            repo.full_name
                .split('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        } else {
            contents_user.clone()
        };
        let repo_name = repo.name.clone();

        let image_url = non_empty(manifest.preview.clone())
            .map(|p| resolve_or_raw(&p, &user, &repo_name, &branch));
        let readme_url = non_empty(manifest.readme.clone())
            .map(|r| resolve_or_raw(&r, &user, &repo_name, &branch));

        let (extension_url, css_url, schemes_url) = match kind {
            ItemKind::Extension => (
                manifest
                    .main
                    .as_deref()
                    .map(|m| resolve_or_raw(m, &user, &repo_name, &branch)),
                None,
                None,
            ),
            ItemKind::Theme => (
                None,
                manifest
                    .usercss
                    .as_deref()
                    .map(|c| resolve_or_raw(c, &user, &repo_name, &branch)),
                manifest
                    .schemes
                    .as_deref()
                    .map(|s| resolve_or_raw(s, &user, &repo_name, &branch)),
            ),
        };

        items.push(CardItem {
            title: manifest.name.clone(),
            subtitle: manifest.description.clone(),
            authors: process_authors(&manifest, &user),
            user,
            repo: repo_name,
            branch,
            image_url,
            readme_url,
            stars: repo.stargazers_count,
            tags: manifest.tags.clone(),
            manifest,
            kind,
            extension_url,
            css_url,
            schemes_url,
        });
    }
    items
}

/// Port of `processAuthors`: fall back to the repo owner, sanitize URLs.
pub fn process_authors(manifest: &Manifest, user: &str) -> Vec<Author> {
    if manifest.authors.is_empty() {
        return vec![Author {
            name: user.to_owned(),
            url: format!("https://github.com/{user}"),
        }];
    }
    manifest
        .authors
        .iter()
        .map(|a| Author {
            name: a.name.clone(),
            url: sanitize_url(&a.url),
        })
        .collect()
}

/// Port of `sanitizeUrl`: neutralize dangerous URL schemes.
pub fn sanitize_url(url: &str) -> String {
    let lower = url.to_lowercase();
    if lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("vbscript:")
    {
        return "about:blank".to_owned();
    }
    url.to_owned()
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::validation::parse_manifest;
    use serde_json::json;

    fn repo() -> Repo {
        serde_json::from_value(json!({
            "name": "my-theme",
            "full_name": "someone/my-theme",
            "html_url": "https://github.com/someone/my-theme",
            "archived": false,
            "stargazers_count": 12,
            "default_branch": "main",
            "contents_url": "https://api.github.com/repos/someone/my-theme/contents/{+path}"
        }))
        .unwrap()
    }

    #[test]
    fn builds_extension_item() {
        let manifest =
            parse_manifest(&json!({ "name": "Ext", "description": "d", "main": "ext.js" }))
                .unwrap();
        let items = build_card_items(&repo(), vec![manifest]);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.kind, ItemKind::Extension);
        assert_eq!(
            item.extension_url.as_deref(),
            Some("https://raw.githubusercontent.com/someone/my-theme/main/ext.js")
        );
        assert_eq!(item.id(), "someone/my-theme#Ext");
        assert_eq!(item.authors[0].name, "someone");
    }

    #[test]
    fn builds_theme_item_with_schemes() {
        let manifest = parse_manifest(&json!({
            "name": "Theme",
            "description": "d",
            "usercss": "user.css",
            "schemes": "https://example.com/color.ini",
            "authors": [{ "name": "alice" }]
        }))
        .unwrap();
        let items = build_card_items(&repo(), vec![manifest]);
        assert_eq!(items[0].kind, ItemKind::Theme);
        assert_eq!(
            items[0].css_url.as_deref(),
            Some("https://raw.githubusercontent.com/someone/my-theme/main/user.css")
        );
        assert_eq!(
            items[0].schemes_url.as_deref(),
            Some("https://example.com/color.ini")
        );
        assert_eq!(items[0].authors[0].url, "https://github.com/alice");
    }

    #[test]
    fn skips_app_manifests() {
        let manifest = parse_manifest(&json!({ "name": "App", "description": "d" })).unwrap();
        assert!(build_card_items(&repo(), vec![manifest]).is_empty());
    }

    #[test]
    fn sanitizes_author_urls() {
        let manifest = parse_manifest(&json!({
            "name": "T",
            "description": "d",
            "usercss": "u.css",
            "authors": [{ "name": "evil", "url": "javascript:alert(1)" }]
        }))
        .unwrap();
        let items = build_card_items(&repo(), vec![manifest]);
        assert_eq!(items[0].authors[0].url, "about:blank");
    }

    #[test]
    fn manifest_branch_overrides_default() {
        let manifest = parse_manifest(&json!({
            "name": "T",
            "description": "d",
            "usercss": "u.css",
            "branch": "dev"
        }))
        .unwrap();
        let items = build_card_items(&repo(), vec![manifest]);
        assert_eq!(items[0].branch, "dev");
        assert_eq!(
            items[0].css_url.as_deref(),
            Some("https://raw.githubusercontent.com/someone/my-theme/dev/u.css")
        );
    }
}
