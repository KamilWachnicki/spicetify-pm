use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// GitHub repository as returned by the search API (subset of fields we use).
#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    pub name: String,
    pub full_name: String,
    pub html_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub archived: bool,
    pub stargazers_count: u64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    #[expect(dead_code)]
    pub pushed_at: String,
    pub default_branch: String,
    #[serde(default)]
    pub contents_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SearchResponse {
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub items: Vec<Repo>,
}

/// A validated marketplace manifest (zod-parity with FetchRemotes.ts).
/// Unknown keys are preserved in `extra` (`.passthrough()`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usercss: Option<String>,
    pub authors: Vec<Author>,
    pub preview: String,
    pub readme: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schemes: Option<String>,
    pub include: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Manifest {
    /// The branch this manifest's files live on:
    /// `manifest.branch || repo default branch`.
    pub fn selected_branch<'a>(&'a self, default_branch: &'a str) -> &'a str {
        match &self.branch {
            Some(b) if !b.is_empty() => b,
            _ => default_branch,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Author {
    pub name: String,
    pub url: String,
}

/// What kind of add-on a manifest describes. Mirrors the discrimination
/// logic across fetchExtensionManifest / fetchThemeManifest / fetchAppManifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Extension,
    Theme,
}

impl ItemKind {
    pub fn from_manifest(manifest: &Manifest) -> Option<ItemKind> {
        if manifest.main.is_some() {
            Some(ItemKind::Extension)
        } else if manifest.usercss.is_some() {
            Some(ItemKind::Theme)
        } else {
            None // custom apps are handled in a later milestone
        }
    }
}

/// Port of the marketplace `CardItem`: one installable entry built from a
/// repo's manifest plus repo metadata.
#[derive(Debug, Clone, Serialize)]
pub struct CardItem {
    pub manifest: Manifest,
    pub kind: ItemKind,
    pub title: String,
    pub subtitle: String,
    pub authors: Vec<Author>,
    pub user: String,
    pub repo: String,
    pub branch: String,
    pub image_url: Option<String>,
    pub readme_url: Option<String>,
    pub stars: u64,
    pub tags: Vec<String>,

    // extension-only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_url: Option<String>,

    // theme-only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub css_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schemes_url: Option<String>,
}

impl CardItem {
    /// spicepm identity key: `{user}/{repo}#{manifest.name}`.
    /// Unique per manifest (names are the identity within a repo), stable
    /// across upstream file renames, and identical to the install target
    /// syntax: `spicepm install <user/repo#Name>`.
    pub fn id(&self) -> String {
        format!("{}/{}#{}", self.user, self.repo, self.manifest.name)
    }

    pub fn github_url(&self) -> String {
        crate::market::urls::github_repo_url(&self.user, &self.repo)
    }
}

/// A CSS snippet from resources/snippets.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub title: String,
    pub description: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

impl Snippet {
    /// Marketplace-style key: spaces replaced by dashes.
    pub fn key(&self) -> String {
        self.title.replace(' ', "-")
    }

    pub fn full_key(&self) -> String {
        format!("snippet:{}", self.key())
    }
}
