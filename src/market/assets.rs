//! Downloading a whole directory for a manifest's spice-pm exclusive
//! `assets` field (fonts, images, ... referenced by a theme's CSS).
//!
//! GitHub has no "download directory" endpoint, so the Git Trees API lists
//! every blob under the target path in one request and each file is then
//! fetched from `raw.githubusercontent.com`. Repo-relative layout is kept
//! so `url(...)` references in the theme CSS keep resolving.
//!
//! Guardrails applied to the listing *before* anything is downloaded:
//! inert file types only ([`ALLOWED_EXTENSIONS`]), no installer-owned
//! names, sane paths, and file count / total size caps.

use super::urls::TreeLocation;
use crate::errors::{Error, Result};
use crate::http::HttpClient;
use futures::stream::{self, StreamExt, TryStreamExt};
use indicatif::ProgressBar;
use serde::Deserialize;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Refuse pathological directories before downloading anything.
const MAX_FILES: usize = 200;
const MAX_TOTAL_BYTES: u64 = 25 * 1024 * 1024;

/// How many asset files to download at once. raw.githubusercontent.com is
/// a CDN without API-style rate limits, so a modest bound cuts the serial
/// per-request latency down by roughly this factor.
const DOWNLOAD_CONCURRENCY: usize = 8;

/// File types an `assets` directory may contain: inert support files only -
/// images, fonts, styles, text/data. Anything that could execute (scripts,
/// binaries, installers) is refused while still at the listing stage, so a
/// hostile manifest can never make spice-pm place a runnable file on disk.
const ALLOWED_EXTENSIONS: [&str; 27] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "avif", // images
    "ttf", "otf", "woff", "woff2", "eot", // fonts
    "css", "scss", "sass", "less", // styles
    "json", "txt", "md", "ini", "xml", "yaml", "yml", "toml", "csv", // text | data
];

/// File names the theme installer owns; an asset with the same destination
/// would clobber them.
const RESERVED_NAMES: [&str; 3] = ["user.css", "color.ini", "theme.js"];

/// One file to install: destination relative to the theme folder, where it
/// came from, and its content. The repo-directory prefix is stripped so
/// files land at the theme folder root - the same flattening `user.css` /
/// `color.ini` / `theme.js` get, and what spicetify's relative `url(...)`
/// lookups from a theme expect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetFile {
    pub rel: String,
    pub url: String,
    pub bytes: Vec<u8>,
}

#[derive(Deserialize)]
struct TreesResponse {
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    tree: Vec<TreeEntry>,
}

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    size: u64,
}

/// A blob selected for download: full repo path (for the raw URL) plus its
/// destination relative to the theme folder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedBlob {
    /// Full path inside the repo; also the raw.githubusercontent.com suffix.
    repo_path: String,
    /// Destination relative to the theme folder (`repo_path` minus the
    /// assets-directory prefix).
    rel: String,
    size: u64,
}

/// Fetch every file under `loc` from GitHub, `DOWNLOAD_CONCURRENCY` files
/// at a time. Fails on any listing or download problem - callers treat
/// assets as required, not best-effort.
pub(crate) async fn fetch_dir(
    http: &HttpClient,
    loc: &TreeLocation,
    progress: Option<&ProgressBar>,
) -> Result<Vec<AssetFile>> {
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
        loc.user, loc.repo, loc.branch
    );
    let response: TreesResponse = http.get_json(&api_url).await?;
    if response.truncated {
        return Err(Error::other(
            "assets directory listing was too large for GitHub to return in full; refusing \
             a partial download",
        ));
    }

    let blobs = select_blobs(&response.tree, &loc.dir_path)?;
    let raw_base = format!(
        "https://raw.githubusercontent.com/{}/{}/{}",
        loc.user, loc.repo, loc.branch
    );
    download_blobs(http, &raw_base, blobs, progress).await
}

/// Download every blob from `raw_base`, up to [`DOWNLOAD_CONCURRENCY`] at
/// a time. The first failure aborts the whole install before anything
/// reaches disk (writes happen only after this returns Ok).
async fn download_blobs(
    http: &HttpClient,
    raw_base: &str,
    blobs: Vec<SelectedBlob>,
    progress: Option<&ProgressBar>,
) -> Result<Vec<AssetFile>> {
    let total = blobs.len();
    if let Some(pb) = progress {
        pb.set_message(format!("0/{total} asset files"));
    }
    let done = AtomicUsize::new(0);
    let done = &done;

    let downloads = blobs.into_iter().map(|blob| async move {
        let url = format!("{raw_base}/{}", blob.repo_path);
        let bytes = http.download(&url).await?;
        if let Some(pb) = progress {
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            pb.set_message(format!("{n}/{total} asset files"));
        }
        Ok::<AssetFile, Error>(AssetFile {
            rel: blob.rel,
            url,
            bytes,
        })
    });
    stream::iter(downloads)
        .buffer_unordered(DOWNLOAD_CONCURRENCY)
        .try_collect::<Vec<AssetFile>>()
        .await
}

/// Pick the blobs under `dir_path` (all blobs when it is empty) and enforce
/// the safety rails: sane paths only, file count and total size caps.
fn select_blobs(entries: &[TreeEntry], dir_path: &str) -> Result<Vec<SelectedBlob>> {
    if let Some(bad) = entries.iter().find(|e| !is_safe_repo_path(&e.path)) {
        return Err(Error::other(format!(
            "assets contain an unsafe path `{}`; refusing",
            bad.path
        )));
    }

    let selected: Vec<SelectedBlob> = entries
        .iter()
        .filter(|e| e.kind == "blob")
        .filter(|e| under_dir(&e.path, dir_path))
        .map(|e| {
            let rel = destination_rel(&e.path, dir_path);
            SelectedBlob {
                repo_path: e.path.clone(),
                rel,
                size: e.size,
            }
        })
        .collect();

    if let Some(clash) = selected
        .iter()
        .find(|b| RESERVED_NAMES.contains(&b.rel.to_lowercase().as_str()))
    {
        return Err(Error::other(format!(
            "asset `{}` collides with a file the theme installer manages",
            clash.repo_path
        )));
    }

    if let Some(bad) = selected.iter().find(|b| !is_allowed_asset(&b.repo_path)) {
        return Err(Error::other(format!(
            "asset `{}` has a file type spice-pm does not allow; assets are limited to inert \
             support files: {}",
            bad.repo_path,
            ALLOWED_EXTENSIONS.join(", ")
        )));
    }

    if selected.is_empty() {
        let reason = if dir_path.is_empty() {
            "assets directory contains no files".to_owned()
        } else {
            format!("no files found under `{dir_path}` in the repo")
        };
        return Err(Error::other(reason));
    }
    if selected.len() > MAX_FILES {
        return Err(Error::other(format!(
            "assets directory has {} files; the limit is {MAX_FILES}",
            selected.len()
        )));
    }
    let total: u64 = selected.iter().map(|b| b.size).sum();
    if total > MAX_TOTAL_BYTES {
        return Err(Error::other(format!(
            "assets directory is {total} bytes in total; the limit is {MAX_TOTAL_BYTES}"
        )));
    }
    Ok(selected)
}

/// Destination for a blob, relative to the theme folder. The assets
/// directory's *parent* prefix is dropped so the dir keeps its own name:
/// `catppuccin/assets/frappe/x.gif` under `catppuccin/assets` lands at
/// `assets/frappe/x.gif` - mirroring how `user.css` is flattened to the
/// theme root while its repo siblings stay siblings. Repo-root assets
/// (empty dir) keep their full path.
fn destination_rel(path: &str, dir_path: &str) -> String {
    if dir_path.is_empty() || !path.starts_with(dir_path) {
        return path.to_owned();
    }
    let basename_start = dir_path.rfind('/').map_or(0, |i| i + 1);
    let rest = path[dir_path.len()..].trim_start_matches('/');
    format!("{}/{}", &dir_path[basename_start..], rest)
}

/// True when the file's extension (of the file name only - dots in parent
/// directories don't count) is on the inert-types allowlist. Extensionless
/// and dotfile entries are refused.
fn is_allowed_asset(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty() && ALLOWED_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        }
        None => false,
    }
}

/// True when `path` sits directly inside `dir_path` (`""` means repo root).
fn under_dir(path: &str, dir_path: &str) -> bool {
    if dir_path.is_empty() {
        return true;
    }
    match path.strip_prefix(dir_path) {
        Some(rest) => rest.starts_with('/') && rest.len() > 1,
        None => false,
    }
}

/// Reject anything that could escape the theme folder: traversal segments,
/// absolute paths, backslashes, NULs.
fn is_safe_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|comp| !comp.is_empty() && comp != "." && comp != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, kind: &str, size: u64) -> TreeEntry {
        TreeEntry {
            path: path.to_owned(),
            kind: kind.to_owned(),
            size,
        }
    }

    #[test]
    fn selects_blobs_under_the_directory_only() {
        let entries = [
            entry("README.md", "blob", 10),
            entry("assets", "tree", 0),
            entry("assets/fonts/a.ttf", "blob", 100),
            entry("assets/img/b.png", "blob", 200),
            // same-name prefix but a sibling, not a child
            entry("assets-old/c.css", "blob", 5),
            // nested deeper: still a child
            entry("assets/deep/d.json", "blob", 7),
        ];
        let picked = select_blobs(&entries, "assets").unwrap();
        assert_eq!(
            picked,
            vec![
                SelectedBlob {
                    repo_path: "assets/fonts/a.ttf".into(),
                    rel: "assets/fonts/a.ttf".into(),
                    size: 100
                },
                SelectedBlob {
                    repo_path: "assets/img/b.png".into(),
                    rel: "assets/img/b.png".into(),
                    size: 200
                },
                SelectedBlob {
                    repo_path: "assets/deep/d.json".into(),
                    rel: "assets/deep/d.json".into(),
                    size: 7
                },
            ]
        );
    }

    #[test]
    fn subdir_assets_keep_their_own_dir_name() {
        // the spicetify-catpuccin case: theme files live in `catppuccin/`
        let entries = [
            entry("catppuccin/user.css", "blob", 1),
            entry(
                "catppuccin/assets/mocha/equalizer-animated-red.gif",
                "blob",
                1,
            ),
        ];
        let picked = select_blobs(&entries, "catppuccin/assets").unwrap();
        assert_eq!(
            picked[0].repo_path,
            "catppuccin/assets/mocha/equalizer-animated-red.gif"
        );
        assert_eq!(picked[0].rel, "assets/mocha/equalizer-animated-red.gif");
    }

    #[test]
    fn empty_dir_selects_everything_at_root_level() {
        let entries = [entry("a.css", "blob", 1), entry("sub/b.txt", "blob", 2)];
        let picked = select_blobs(&entries, "").unwrap();
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].rel, "a.css");
        assert_eq!(picked[1].rel, "sub/b.txt");
    }

    #[test]
    fn installer_owned_names_are_protected() {
        // collisions are only possible at theme-root level: repo-root
        // assets (e.g. `assets` = bare `tree/<branch>` URL)
        let entries = [
            entry("user.css", "blob", 1),
            entry("color.ini", "blob", 1),
            entry("sub/theme.js", "blob", 1), // nested: fine
        ];
        let err = select_blobs(&entries, "").unwrap_err();
        assert!(err.to_string().contains("collides"), "{err}");

        // nested copies of managed names are allowed - they land in their
        // own subfolder and clobber nothing
        let ok = [entry("catppuccin/assets/img.png", "blob", 1)];
        assert!(select_blobs(&ok, "catppuccin/assets").is_ok());
    }

    #[test]
    fn empty_selection_is_an_error() {
        let entries = [entry("elsewhere/x", "blob", 1)];
        assert!(select_blobs(&entries, "assets").is_err());
        assert!(select_blobs(&[], "").is_err());
    }

    #[test]
    fn limits_are_enforced_before_download() {
        let many: Vec<TreeEntry> = (0..=MAX_FILES)
            .map(|i| entry(format!("assets/f{i}.png").as_str(), "blob", 1))
            .collect();
        let err = select_blobs(&many, "assets").unwrap_err();
        assert!(err.to_string().contains("files"), "{err}");

        let entries = [
            entry("assets/big.png", "blob", MAX_TOTAL_BYTES),
            entry("assets/over.png", "blob", 1),
        ];
        let err = select_blobs(&entries, "assets").unwrap_err();
        assert!(err.to_string().contains("bytes"), "{err}");
    }

    #[test]
    fn only_inert_file_types_are_allowed() {
        // the whole allowlist passes
        let allowed = [
            "assets/a.png",
            "assets/b.jpeg",
            "assets/c.gif",
            "assets/d.webp",
            "assets/e.svg",
            "assets/f.ico",
            "assets/g.avif",
            "assets/h.ttf",
            "assets/i.OTF",
            "assets/j.woff2",
            "assets/k.css",
            "assets/l.scss",
            "assets/m.json",
            "assets/n.txt",
            "assets/o.md",
            "assets/p.ini",
            "assets/q.xml",
            "assets/r.yaml",
        ];
        let entries: Vec<TreeEntry> = allowed
            .iter()
            .enumerate()
            .map(|(i, p)| entry(p, "blob", i as u64 + 1))
            .collect();
        assert!(
            select_blobs(&entries, "assets").is_ok(),
            "all inert types pass"
        );

        // anything that could run is refused before download
        for bad in [
            "assets/run.exe",
            "assets/run.SH",
            "assets/run.bat",
            "assets/run.cmd",
            "assets/run.ps1",
            "assets/lib.dll",
            "assets/lib.so",
            "assets/lib.dylib",
            "assets/x.msi",
            "assets/x.jar",
            "assets/script.js",
            "assets/script.py",
            "assets/no-extension",
            "assets/.dotfile",
            // dots in parent directories must not confuse the check
            "assets/v1.2/x.exe",
        ] {
            let entries = [entry(bad, "blob", 1)];
            let err = select_blobs(&entries, "assets").unwrap_err();
            assert!(
                err.to_string().contains("does not allow"),
                "`{bad}` must be refused: {err}"
            );
        }
    }

    #[test]
    fn unsafe_paths_are_refused() {
        let entries = [
            entry("assets/ok.ttf", "blob", 1),
            entry("../escape.ttf", "blob", 1),
        ];
        let err = select_blobs(&entries, "assets").unwrap_err();
        assert!(err.to_string().contains("unsafe"), "{err}");

        for bad in ["../x", "/abs", "a\\\\b", "a/../b", "a//b", "a/", "."] {
            assert!(!is_safe_repo_path(bad), "`{bad}` must be unsafe");
        }
        assert!(is_safe_repo_path("assets/fonts/a.ttf"));
    }

    /// Wiremock-backed checks for the concurrent download loop.
    mod net {
        use super::*;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn blob(repo_path: &str) -> SelectedBlob {
            SelectedBlob {
                repo_path: repo_path.to_owned(),
                rel: destination_rel(repo_path, "assets"),
                size: 1,
            }
        }

        #[tokio::test]
        async fn downloads_every_file_under_concurrency() {
            let server = MockServer::start().await;
            let files = ["a.png", "b.gif", "sub/c.webp"];
            for (i, name) in files.iter().enumerate() {
                let body = vec![u8::try_from(i).unwrap(); 5];
                Mock::given(method("GET"))
                    .and(path(format!("/assets/{name}")))
                    .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
                    .expect(1)
                    .mount(&server)
                    .await;
            }

            let http = HttpClient::new_with_cache(
                false,
                tempfile::tempdir().unwrap().path().to_path_buf(),
            )
            .unwrap();
            let blobs: Vec<_> = files.iter().map(|f| blob(&format!("assets/{f}"))).collect();
            let got = download_blobs(&http, &server.uri(), blobs, None)
                .await
                .unwrap();

            let mut by_rel: Vec<(String, Vec<u8>)> =
                got.into_iter().map(|f| (f.rel, f.bytes)).collect();
            by_rel.sort();
            assert_eq!(
                by_rel,
                vec![
                    ("assets/a.png".to_owned(), vec![0; 5]),
                    ("assets/b.gif".to_owned(), vec![1; 5]),
                    ("assets/sub/c.webp".to_owned(), vec![2; 5]),
                ]
            );
        }

        #[tokio::test]
        async fn any_failed_download_aborts_the_whole_fetch() {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/assets/ok.png"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
                .expect(1)
                .mount(&server)
                .await;
            // 404: fails immediately, no retry storm
            Mock::given(method("GET"))
                .and(path("/assets/gone.png"))
                .respond_with(ResponseTemplate::new(404))
                .expect(1)
                .mount(&server)
                .await;

            let http = HttpClient::new_with_cache(
                false,
                tempfile::tempdir().unwrap().path().to_path_buf(),
            )
            .unwrap();
            let blobs = vec![blob("assets/ok.png"), blob("assets/gone.png")];
            let err = download_blobs(&http, &server.uri(), blobs, None)
                .await
                .unwrap_err();
            assert!(err.to_string().contains("not found"), "{err}");
        }
    }
}
