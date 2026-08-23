//! Blacklist handling, ported from the marketplace's
//! `matchesBlacklistPattern` / `isBlacklisted`:
//!
//! - matching is case-insensitive
//! - patterns without `*` must equal the repo `html_url` exactly
//! - in glob patterns each `*` matches one or more characters, none of which
//!   may be `/` (the web app compiles `*` to regex `[^/]+`)
//! - non-URL entries (comment lines) simply never match anything

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BlacklistFile {
    #[serde(default)]
    pub repos: Vec<String>,
}

/// Fetch blacklist.json through the TTL cache.
pub async fn fetch_blacklist(
    http: &crate::http::HttpClient,
    ttl_secs: u64,
) -> crate::errors::Result<Blacklist> {
    let file: BlacklistFile = http
        .get_json_cached(crate::market::constants::BLACKLIST_URL, ttl_secs)
        .await?;
    Ok(Blacklist::from_repos(file.repos))
}

#[derive(Debug, Clone, Default)]
pub struct Blacklist {
    patterns: Vec<String>,
}

impl Blacklist {
    pub fn from_repos(repos: Vec<String>) -> Self {
        Self { patterns: repos }
    }

    pub fn is_blacklisted(&self, url: &str) -> bool {
        self.patterns.iter().any(|p| matches_pattern(url, p))
    }
}

fn matches_pattern(url: &str, pattern: &str) -> bool {
    let url = url.to_lowercase();
    let pattern = pattern.to_lowercase();
    if !pattern.contains('*') {
        return url == pattern;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    match_glob(&parts, &url)
}

/// Anchored glob where every `*` (the gap between consecutive parts) matches
/// at least one character and none of them may be a `/`.
fn match_glob(parts: &[&str], text: &str) -> bool {
    let Some((head, tail)) = parts.split_first() else {
        return text.is_empty();
    };
    if !text.starts_with(head) {
        return false;
    }
    if tail.is_empty() {
        return text.len() == head.len();
    }
    match_star(tail, &text[head.len()..])
}

/// Match `* <part> ...` against text: the star consumes >= 1 slash-free char.
fn match_star(parts: &[&str], text: &str) -> bool {
    let Some(part) = parts.first() else {
        // nothing after this star; the star still had to consume something,
        // but that was validated by the caller before recursing here only
        // when more parts existed - reaching here means empty remainder.
        return false;
    };
    for (start, _) in text.match_indices(part) {
        if start == 0 {
            continue; // star must consume at least one character
        }
        if text[..start].contains('/') {
            break; // later occurrences would also span a '/'
        }
        let rest = &text[start + part.len()..];
        if parts.len() == 1 {
            if rest.is_empty() {
                return true;
            }
        } else if match_star(&parts[1..], rest) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bl(patterns: &[&str]) -> Blacklist {
        Blacklist::from_repos(
            patterns
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        )
    }

    #[test]
    fn exact_match_is_case_insensitive() {
        let b = bl(&["https://github.com/FoxRefire/spiceDL"]);
        assert!(b.is_blacklisted("https://github.com/FoxRefire/spiceDL"));
        assert!(b.is_blacklisted("https://github.com/foxrefire/spicedl"));
        assert!(!b.is_blacklisted("https://github.com/FoxRefire/other"));
    }

    #[test]
    fn user_glob_matches_single_segment_only() {
        let b = bl(&["https://github.com/FoxRefire/*"]);
        assert!(b.is_blacklisted("https://github.com/FoxRefire/spiceDL"));
        assert!(!b.is_blacklisted("https://github.com/FoxRefire/a/b"));
        assert!(!b.is_blacklisted("https://github.com/FoxRefire"));
    }

    #[test]
    fn repo_glob_matches_any_user() {
        let b = bl(&["https://github.com/*/Speedify"]);
        assert!(b.is_blacklisted("https://github.com/s000ik/Speedify"));
        assert!(b.is_blacklisted("https://github.com/ssatwik975/Speedify"));
        assert!(!b.is_blacklisted("https://github.com/a/b/Speedify"));
        assert!(!b.is_blacklisted("https://github.com/a/Speedify/x"));
    }

    #[test]
    fn comment_lines_never_match() {
        let b = bl(&["// for old versions:", "// new bl syntax"]);
        assert!(!b.is_blacklisted("https://github.com/anything/here"));
    }

    #[test]
    fn multiple_stars() {
        let b = bl(&["https://github.com/*/*/blocked*repo"]);
        assert!(b.is_blacklisted("https://github.com/u/r/blocked-repo"));
        assert!(!b.is_blacklisted("https://github.com/u/blocked-repo"));
        assert!(!b.is_blacklisted("https://github.com/u/r/sub/blocked-repo"));
    }

    #[test]
    fn parses_blacklist_json() {
        let file: BlacklistFile =
            serde_json::from_str(r#"{ "repos": ["https://github.com/a/b", "not a url"] }"#)
                .unwrap();
        assert_eq!(file.repos.len(), 2);
    }
}
