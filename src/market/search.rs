//! Topic search, ported from `getTaggedRepos`.

use super::blacklist::Blacklist;
use super::constants::{MANIFEST_TTL_SECS, RepoTopic, SEARCH_TTL_SECS, search_repos_url};
use super::types::{Repo, SearchResponse};
use crate::errors::Result;
use crate::http::HttpClient;

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub topic: RepoTopic,
    pub page: u32,
    pub include_archived: bool,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub items: Vec<Repo>,
    /// Count of items returned by GitHub before rule filtering.
    #[expect(dead_code)]
    pub page_count: usize,
    pub total_count: u64,
    pub blacklisted_filtered: usize,
    pub archived_filtered: usize,
}

/// Query GitHub for all repos tagged with the requested topic and filter
/// them through the blacklist + archive rules.
pub async fn get_tagged_repos(
    http: &HttpClient,
    blacklist: &Blacklist,
    opts: SearchOptions,
) -> Result<SearchResult> {
    let url = search_repos_url(opts.topic.as_str(), opts.page.max(1));
    let response: SearchResponse = http.get_json_cached(&url, SEARCH_TTL_SECS).await?;

    let mut items = Vec::new();
    let mut blacklisted_filtered = 0usize;
    let mut archived_filtered = 0usize;
    for repo in response.items {
        if blacklist.is_blacklisted(&repo.html_url) {
            blacklisted_filtered += 1;
            continue;
        }
        if repo.archived && !opts.include_archived {
            archived_filtered += 1;
            continue;
        }
        items.push(repo);
    }

    Ok(SearchResult {
        page_count: items.len(),
        total_count: response.total_count,
        items,
        blacklisted_filtered,
        archived_filtered,
    })
}

/// Fetch a single repository's metadata by `user/repo`
/// (used by `info` / direct installs).
pub async fn get_repo(http: &HttpClient, user: &str, repo: &str) -> Result<Repo> {
    let url = format!("https://api.github.com/repos/{user}/{repo}");
    http.get_json_cached(&url, MANIFEST_TTL_SECS).await
}
