//! Ledger of everything spice-pm installed, stored at
//! `<SPICETIFY_CONFIG>/spicepm/ledger.json`. Tracks provenance so
//! `update` / `remove` / `list` work reliably.
//!
//! Identity scheme (v2):
//! - extensions/themes: `{user}/{repo}#{manifest.name}` - mirrors the
//!   `spice-pm install user/repo#Name` target syntax, unique per manifest
//!   (names are the identity within a repo) and stable across upstream
//!   file renames
//! - snippet group: `@spicepm/snippets` - reserved `@` namespace, which
//!   GitHub usernames cannot contain, so it never collides with a real repo

// note: no cross-version migration - if the format changes, re-install.

use crate::errors::Result;
use crate::spicetify::dirs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SNIPPET_GROUP_ID: &str = "@spicepm/snippets";

/// Schema version stamped at the root of the ledger file.
pub const LEDGER_VERSION: u32 = 1;

fn default_ledger_version() -> u32 {
    LEDGER_VERSION
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Extension,
    Theme,
    SnippetGroup,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Unique, meaningful identity (see module docs).
    pub id: String,
    pub kind: Kind,
    pub user: String,
    pub repo: String,
    pub branch: String,
    /// Files written relative to the spicetify config dir.
    pub files: Vec<String>,
    /// Entries added to config-xpui.ini (e.g. extension basenames,
    /// theme folder name).
    pub config_refs: Vec<String>,
    pub resolved_urls: Vec<String>,
    /// sha256 per file (relative path).
    pub hashes: BTreeMap<String, String>,
    /// Enabled snippet keys (`SnippetGroup` entries only).
    #[serde(default)]
    pub snippet_keys: Vec<String>,
    pub installed_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Ledger {
    /// Schema version; see [`LEDGER_VERSION`].
    #[serde(default = "default_ledger_version")]
    pub version: u32,
    pub entries: Vec<LedgerEntry>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: LEDGER_VERSION,
            entries: Vec::new(),
        }
    }
}

impl Ledger {
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.is_file() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::cache::atomic_write(&path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn path() -> PathBuf {
        dirs::spicepm_data_dir().join("ledger.json")
    }

    /// Ids of all installed entries - used to mark search results.
    pub fn installed_ids() -> std::collections::HashSet<String> {
        Self::load()
            .map(|l| l.entries.into_iter().map(|e| e.id).collect())
            .unwrap_or_default()
    }

    pub fn find(&self, id: &str) -> Option<&LedgerEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn upsert(&mut self, entry: LedgerEntry) {
        self.entries.retain(|e| e.id != entry.id);
        self.entries.push(entry);
    }

    pub fn remove(&mut self, id: &str) -> Option<LedgerEntry> {
        let idx = self.entries.iter().position(|e| e.id == id)?;
        Some(self.entries.remove(idx))
    }
}

/// sha256 of a byte slice as lowercase hex.
pub fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Resolve a file path recorded in the ledger (relative to the spicetify
/// config dir) and reject anything that escapes it.
pub fn resolve_relative(rel: &str) -> Result<PathBuf> {
    // validate the relative part itself; callers always pass paths that are
    // meant to sit below the spicetify dir
    let mut depth = 0i32;
    for comp in Path::new(rel).components() {
        match comp {
            std::path::Component::ParentDir => depth -= 1,
            std::path::Component::Normal(_) => depth += 1,
            _ => {}
        }
        if depth < 0 {
            return Err(crate::errors::Error::other(format!(
                "refusing unsafe path `{rel}`"
            )));
        }
    }
    Ok(dirs::spicetify_dir().join(rel))
}

pub fn rel_from_base(path: &Path) -> String {
    path.strip_prefix(dirs::spicetify_dir())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_entries_serialize_without_legacy_fields() {
        let entry = LedgerEntry {
            id: "Comfy-Themes/Spicetify#Comfy".to_owned(),
            kind: Kind::Theme,
            user: "Comfy-Themes".to_owned(),
            repo: "Spicetify".to_owned(),
            branch: "main".to_owned(),
            files: vec!["Themes/Comfy/user.css".to_owned()],
            installed_at: 1_750_000_000,
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&entry).unwrap();
        assert!(
            !json.contains("manifest_name"),
            "legacy field must stay dead"
        );
        assert!(
            !json.contains("version"),
            "entries carry no version: {json}"
        );
    }

    #[test]
    fn root_ledger_carries_version() {
        let led = Ledger::default();
        assert_eq!(led.version, LEDGER_VERSION);
        let json = serde_json::to_string_pretty(&led).unwrap();
        assert!(json.contains("\"version\": 1"), "{json}");
    }

    #[test]
    fn upsert_replaces_same_id() {
        let mut led = Ledger::default();
        let e = LedgerEntry {
            id: "u/r#N".to_owned(),
            installed_at: 1,
            ..Default::default()
        };
        led.upsert(e.clone());
        led.upsert(e);
        assert_eq!(led.entries.len(), 1);
    }

    #[test]
    fn find_and_remove_by_id() {
        let mut led = Ledger::default();
        led.upsert(LedgerEntry {
            id: "u/r#N".to_owned(),
            ..Default::default()
        });
        assert!(led.find("u/r#N").is_some());
        assert!(led.find("u/r#Other").is_none());
        assert!(led.remove("u/r#N").is_some());
        assert!(led.remove("u/r#N").is_none());
    }

    #[test]
    fn resolve_relative_rejects_traversal() {
        assert!(resolve_relative("Themes/ok/user.css").is_ok());
        assert!(resolve_relative("../escape").is_err());
    }
}
