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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLocation {
    pub user: String,
    pub repo: String,
    pub branch: String,
    /// Path of the directory inside the repo; empty means the repo root.
    pub dir_path: String,
}

/// Parse a github.com directory URL into its parts:
/// `https://github.com/{user}/{repo}/tree/{branch}[/{dir}]`.
///
/// Like [`parse_github_raw`], the first segment after `tree/` is taken as
/// the branch; repos with slashes in the branch name should use a
/// repo-relative path instead of an absolute URL.
pub fn parse_github_tree(url: &str) -> Option<TreeLocation> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut segments = rest.splitn(4, '/');
    let user = segments.next()?;
    let repo = segments.next()?;
    if segments.next()? != "tree" {
        return None; // blob = file, anything else is not a directory
    }
    let remainder = segments.next()?;
    if user.is_empty() || repo.is_empty() || remainder.is_empty() {
        return None;
    }
    let (branch, dir_path) = match remainder.split_once('/') {
        Some((branch, dir)) => (branch, normalize_repo_path(dir)?),
        None => (remainder, String::new()),
    };
    if branch.is_empty() {
        return None;
    }
    Some(TreeLocation {
        user: user.to_owned(),
        repo: repo.to_owned(),
        branch: branch.to_owned(),
        dir_path,
    })
}

/// Resolve a manifest `assets` value to a concrete repo location: absolute
/// URLs must be `github.com/.../tree/...` directory links (anything else
/// cannot be listed); bare values are repo-relative paths on the item's own
/// branch.
pub fn resolve_assets_dir(
    spec: &str,
    user: &str,
    repo: &str,
    branch: &str,
) -> Result<TreeLocation, String> {
    if spec.starts_with("http") {
        parse_github_tree(spec).ok_or_else(|| {
            format!(
                "`assets` must be a https://github.com/<user>/<repo>/tree/<branch>/<dir> URL \
                 or a repo-relative path; got `{spec}`"
            )
        })
    } else {
        let dir_path =
            normalize_repo_path(spec).ok_or_else(|| format!("invalid `assets` path `{spec}`"))?;
        Ok(TreeLocation {
            user: user.to_owned(),
            repo: repo.to_owned(),
            branch: branch.to_owned(),
            dir_path,
        })
    }
}

/// The canonical github.com URL for a tree location (used for provenance).
pub fn github_tree_url(loc: &TreeLocation) -> String {
    let base = format!(
        "https://github.com/{}/{}/tree/{}",
        loc.user, loc.repo, loc.branch
    );
    if loc.dir_path.is_empty() {
        base
    } else {
        format!("{base}/{}", loc.dir_path)
    }
}

/// Normalize a repo-relative path: backslashes become slashes, `.` and
/// empty segments are dropped, traversal is rejected. Returns None when
/// nothing sane remains or the path escapes.
fn normalize_repo_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for comp in normalized.split('/') {
        match comp {
            "" | "." => {}
            ".." => return None,
            _ => parts.push(comp),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
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

    #[test]
    fn parses_github_tree_urls() {
        let loc = parse_github_tree("https://github.com/u/r/tree/main/assets/fonts").unwrap();
        assert_eq!(
            loc,
            TreeLocation {
                user: "u".into(),
                repo: "r".into(),
                branch: "main".into(),
                dir_path: "assets/fonts".into()
            }
        );
        // bare tree root: whole repo
        let loc = parse_github_tree("https://github.com/u/r/tree/main").unwrap();
        assert_eq!(loc.branch, "main");
        assert_eq!(loc.dir_path, "");
        // non-directory URLs are rejected
        assert!(parse_github_tree("https://github.com/u/r/blob/main/a.css").is_none());
        assert!(parse_github_tree("https://github.com/u/r").is_none());
        assert!(parse_github_tree("https://example.com/u/r/tree/main/x").is_none());
        // traversal inside the URL is rejected
        assert!(parse_github_tree("https://github.com/u/r/tree/main/../..").is_none());
    }

    #[test]
    fn resolves_assets_specs() {
        // relative path against the item's own repo/branch
        let loc = resolve_assets_dir("assets", "u", "r", "dev").unwrap();
        assert_eq!(loc.dir_path, "assets");
        assert_eq!(loc.branch, "dev");
        // windows separators and noise segments normalize away
        let loc = resolve_assets_dir(".\\assets\\./fonts", "u", "r", "main").unwrap();
        assert_eq!(loc.dir_path, "assets/fonts");
        // absolute tree URL passes through
        let loc = resolve_assets_dir(
            "https://github.com/other/r/tree/main/pics",
            "u",
            "r",
            "main",
        )
        .unwrap();
        assert_eq!(loc.user, "other");
        assert_eq!(loc.dir_path, "pics");
        // non-listable URL shapes are rejected with a reason
        assert!(resolve_assets_dir("https://cdn.example.com/dir", "u", "r", "main").is_err());
        assert!(resolve_assets_dir("../escape", "u", "r", "main").is_err());
        assert!(resolve_assets_dir("", "u", "r", "main").is_err());
    }
}
