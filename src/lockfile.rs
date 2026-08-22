//! `spicepm.lock` — a shareable snapshot of everything installed, so a
//! fresh machine can be brought up with a single zero-arg
//! `spicepm install`.

use crate::errors::{Error, Result};
use crate::ledger::{Kind, Ledger};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(test)]
use tempfile::tempdir;

pub const LOCKFILE_VERSION: u32 = 1;
pub const DEFAULT_FILENAME: &str = "spicepm.lock";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub items: Vec<LockItem>,
    #[serde(default)]
    pub snippets: Vec<String>,
}

fn default_version() -> u32 {
    LOCKFILE_VERSION
}

/// One pinned extension/theme: identity plus the branch and colour scheme
/// to restore on a clean machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockItem {
    pub kind: Kind,
    pub id: String,
    pub user: String,
    pub repo: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

impl Lockfile {
    /// Snapshot the live ledger + enabled snippet keys.
    pub fn from_ledger(led: &Ledger, enabled_snippets: &[String]) -> Self {
        let items = led
            .entries
            .iter()
            .filter(|e| matches!(e.kind, Kind::Extension | Kind::Theme))
            .map(|e| {
                let scheme = if e.kind == Kind::Theme {
                    crate::commands::active_theme_scheme(e)
                } else {
                    None
                };
                LockItem {
                    kind: e.kind,
                    id: e.id.clone(),
                    user: e.user.clone(),
                    repo: e.repo.clone(),
                    branch: e.branch.clone(),
                    scheme,
                }
            })
            .collect();
        Self {
            version: LOCKFILE_VERSION,
            items,
            snippets: enabled_snippets.to_vec(),
        }
    }

    pub fn load(path: &std::path::Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn store(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        crate::cache::atomic_write(path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

/// Default lockfile location: next to the ledger inside the spicetify
/// config dir (`<SPICETIFY_CONFIG>/spicepm/spicepm.lock`).
pub fn default_path() -> PathBuf {
    crate::spicetify::dirs::spicepm_data_dir().join(DEFAULT_FILENAME)
}

/// Resolve which lockfile an operation should touch:
/// explicit flag > default location (when present). Missing both points at
/// `spicepm lock`.
pub fn resolve_install_path(flag: Option<&std::path::Path>) -> Result<PathBuf> {
    resolve_in(flag.map(std::convert::AsRef::as_ref), &default_path())
}

fn resolve_in(flag: Option<&std::path::Path>, primary: &std::path::Path) -> Result<PathBuf> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    if primary.is_file() {
        return Ok(primary.to_path_buf());
    }
    Err(Error::other(format!(
        "no lockfile found at {}; run `spicepm lock` first or pass --lockfile <path>",
        primary.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "version": 1,
  "items": [
    {
      "kind": "theme",
      "id": "Comfy-Themes/Spicetify#Comfy",
      "user": "Comfy-Themes",
      "repo": "Spicetify",
      "branch": "main",
      "scheme": "Comfy"
    }
  ],
  "snippets": ["Hamsters-Dancing"]
}"#;

    #[test]
    fn roundtrip_preserves_everything() {
        let lf: Lockfile = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(lf.version, 1);
        assert_eq!(lf.items.len(), 1);
        assert_eq!(lf.items[0].id, "Comfy-Themes/Spicetify#Comfy");
        assert_eq!(lf.items[0].scheme.as_deref(), Some("Comfy"));
        assert_eq!(lf.snippets, ["Hamsters-Dancing"]);

        let out = serde_json::to_string_pretty(&lf).unwrap();
        let reparsed: Lockfile = serde_json::from_str(&out).unwrap();
        assert_eq!(reparsed.items[0].id, lf.items[0].id);
        assert_eq!(reparsed.snippets, lf.snippets);
    }

    #[test]
    fn empty_lockfile_defaults() {
        let lf: Lockfile = serde_json::from_str("{}").unwrap();
        assert_eq!(lf.version, LOCKFILE_VERSION);
        assert!(lf.items.is_empty());
        assert!(lf.snippets.is_empty());
    }

    #[test]
    fn from_ledger_maps_kinds_and_skips_snippet_group() {
        use crate::ledger::{Ledger, LedgerEntry};

        let mut led = Ledger::default();
        led.upsert(LedgerEntry {
            id: "u/r#Ext".to_owned(),
            kind: Kind::Extension,
            user: "u".to_owned(),
            repo: "r".to_owned(),
            branch: "main".to_owned(),
            files: vec!["Extensions/e.js".to_owned()],
            installed_at: 1,
            ..Default::default()
        });
        led.upsert(LedgerEntry {
            id: crate::ledger::SNIPPET_GROUP_ID.to_owned(),
            kind: Kind::SnippetGroup,
            user: "spicepm".to_owned(),
            repo: "snippets".to_owned(),
            installed_at: 2,
            ..Default::default()
        });

        let lf = Lockfile::from_ledger(&led, &["A".to_owned()]);
        assert_eq!(lf.items.len(), 1);
        assert_eq!(lf.items[0].kind, Kind::Extension);
        assert_eq!(lf.snippets, ["A"]);
    }

    #[test]
    fn resolve_prefers_flag_then_primary_when_present() {
        use super::resolve_in;
        let dir = tempdir().unwrap();
        let primary = dir.path().join(DEFAULT_FILENAME);

        // no flag + missing primary -> error pointing at spicepm lock
        assert!(resolve_in(None, &primary).is_err());

        // primary present -> used
        std::fs::write(&primary, "{}").unwrap();
        assert_eq!(resolve_in(None, &primary).unwrap(), primary);

        // explicit flag always wins
        let custom = dir.path().join("custom.lock");
        assert_eq!(resolve_in(Some(&custom), &primary).unwrap(), custom);
    }
}
