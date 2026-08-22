/// Resolve a manifest field to a fetchable URL, mirroring the marketplace:
/// values starting with `http` are used verbatim, everything else is joined
/// onto `raw.githubusercontent.com/{user}/{repo}/{branch}/{path}`.
pub fn resolve_or_raw(value: &str, user: &str, repo: &str, branch: &str) -> String {
    if value.starts_with("http") {
        value.to_owned()
    } else {
        format!("https://raw.githubusercontent.com/{user}/{repo}/{branch}/{value}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLocation {
    pub user: String,
    pub repo: String,
    pub branch: String,
    pub file_path: String,
}

/// Parse a raw.githubusercontent.com URL into its parts
/// (port of `getParamsFromGithubRaw`).
pub fn parse_github_raw(url: &str) -> Option<RawLocation> {
    let rest = url.strip_prefix("https://raw.githubusercontent.com/")?;
    let mut segments = rest.splitn(4, '/');
    let user = segments.next()?;
    let repo = segments.next()?;
    let branch = segments.next()?;
    let file_path = segments.next()?;
    if user.is_empty() || repo.is_empty() || branch.is_empty() || file_path.is_empty() {
        return None;
    }
    Some(RawLocation {
        user: user.to_owned(),
        repo: repo.to_owned(),
        branch: branch.to_owned(),
        file_path: file_path.to_owned(),
    })
}

/// Extract (user, repo) from a GitHub API `contents_url`
/// e.g. `https://api.github.com/repos/theRealPadster/spicetify-hide-podcasts/contents/{+path}`.
/// Port of the regex in FetchRemotes.ts.
pub fn parse_contents_url(contents_url: &str) -> Option<(String, String)> {
    let rest = contents_url.strip_prefix("https://api.github.com/repos/")?;
    let mut segments = rest.split('/');
    let user = segments.next()?;
    let repo = segments.next()?;
    if user.is_empty() || repo.is_empty() {
        return None;
    }
    Some((user.to_owned(), repo.to_owned()))
}

/// Extract the repo `html_url` (`https://github.com/{user}/{repo}`) from a
/// GitHub API repo object's fields.
pub fn github_repo_url(user: &str, repo: &str) -> String {
    format!("https://github.com/{user}/{repo}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_absolute_urls_verbatim() {
        assert_eq!(
            resolve_or_raw("https://cdn.example.com/x.css", "u", "r", "main"),
            "https://cdn.example.com/x.css"
        );
    }

    #[test]
    fn resolve_relative_paths_against_raw() {
        assert_eq!(
            resolve_or_raw("theme/user.css", "u", "r", "main"),
            "https://raw.githubusercontent.com/u/r/main/theme/user.css"
        );
        // marketplace checks `startsWith("http")`, so any scheme prefix counts
        assert_eq!(
            resolve_or_raw("httpx/not-a-url", "u", "r", "b"),
            "httpx/not-a-url"
        );
    }

    #[test]
    fn parse_github_raw_parts() {
        let loc = parse_github_raw(
            "https://raw.githubusercontent.com/spicetify/spicetify-extensions/main/featureshuffle/featureshuffle.js",
        )
        .unwrap();
        assert_eq!(loc.user, "spicetify");
        assert_eq!(loc.repo, "spicetify-extensions");
        assert_eq!(loc.branch, "main");
        assert_eq!(loc.file_path, "featureshuffle/featureshuffle.js");
        assert!(parse_github_raw("https://example.com/a/b/c/d").is_none());
    }

    #[test]
    fn parse_contents_url_parts() {
        let (user, repo) = parse_contents_url(
            "https://api.github.com/repos/theRealPadster/spicetify-hide-podcasts/contents/{+path}",
        )
        .unwrap();
        assert_eq!(user, "theRealPadster");
        assert_eq!(repo, "spicetify-hide-podcasts");
    }
}
