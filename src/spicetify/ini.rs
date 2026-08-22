//! Reading and editing `config-xpui.ini` with the same semantics as
//! `spicetify config`: `extensions` / `custom_apps` are pipe-separated lists
//! in `[AdditionalOptions]`; `current_theme` / `color_scheme` are strings in
//! `[Setting]`. Writes are atomic.

use crate::errors::{Error, Result};
use ini::Ini;
use std::path::{Path, PathBuf};

const SETTING: &str = "Setting";
const ADDITIONAL_OPTIONS: &str = "AdditionalOptions";

pub struct SpicetifyConfig {
    ini: Ini,
    path: PathBuf,
}

impl SpicetifyConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.is_file() {
            return Err(Error::SpicetifyConfigNotFound {
                path: path.display().to_string(),
            });
        }
        let ini = Ini::load_from_file(&path)?;
        Ok(Self { ini, path })
    }

    pub fn get_string(&self, section: &str, key: &str) -> Option<String> {
        self.ini
            .section(Some(section))
            .and_then(|s| s.get(key))
            .map(str::to_owned)
    }

    pub fn set_string(&mut self, section: &str, key: &str, value: &str) {
        self.ini.with_section(Some(section)).set(key, value);
    }

    pub fn current_theme(&self) -> Option<String> {
        self.get_string(SETTING, "current_theme")
    }

    pub fn color_scheme(&self) -> Option<String> {
        self.get_string(SETTING, "color_scheme")
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn extensions(&self) -> Vec<String> {
        self.list_get("extensions")
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn custom_apps(&self) -> Vec<String> {
        self.list_get("custom_apps")
    }

    /// Append to a pipe-list without duplicating (mirrors marketplace's
    /// `addExtensionToSpicetifyConfig`). Returns true when changed.
    pub fn list_add(&mut self, key: &str, value: &str) -> bool {
        let mut items = self.list_get(key);
        if items.iter().any(|i| i == value) {
            return false;
        }
        items.push(value.to_owned());
        self.list_set(key, &items);
        true
    }

    /// Remove from a pipe-list. Returns true when changed.
    pub fn list_remove(&mut self, key: &str, value: &str) -> bool {
        let items = self.list_get(key);
        if !items.iter().any(|i| i == value) {
            return false;
        }
        let filtered: Vec<String> = items.into_iter().filter(|i| i != value).collect();
        self.list_set(key, &filtered);
        true
    }

    fn list_get(&self, key: &str) -> Vec<String> {
        self.get_string(ADDITIONAL_OPTIONS, key)
            .map(|v| {
                v.split('|')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn list_set(&mut self, key: &str, items: &[String]) {
        self.set_string(ADDITIONAL_OPTIONS, key, &items.join("|"));
    }

    pub fn save(&self) -> Result<()> {
        atomic_save(&self.path, &self.ini)
    }
}

fn atomic_save(path: &Path, ini: &Ini) -> Result<()> {
    let tmp = path.with_extension("ini.spicepm-tmp");
    // rust-ini preserves section/key order on write
    ini.write_to_file(&tmp)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_config() -> String {
        r"
        [Setting]
        spotify_path = /opt/spotify/spotify
        current_theme = SpicetifyDefault
        color_scheme =
        inject_css = 1

        [Preprocesses]
        disable_sentry = 1

        [AdditionalOptions]
        extensions =
        custom_apps = new-releases|reddit
        sidebar_config = 0"
            .to_owned()
    }

    fn load_from_str(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config-xpui.ini");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (dir, path)
    }

    #[test]
    fn reads_strings_and_lists() {
        let (_dir, path) = load_from_str(&sample_config());
        let cfg = SpicetifyConfig::load(&path).unwrap();
        assert_eq!(cfg.current_theme().as_deref(), Some("SpicetifyDefault"));
        // `color_scheme =` is set-but-empty in the file
        assert_eq!(cfg.color_scheme().as_deref(), Some(""));
        assert_eq!(cfg.custom_apps(), ["new-releases", "reddit"]);
        assert!(cfg.extensions().is_empty());
    }

    #[test]
    fn list_add_dedupes() {
        let (_dir, path) = load_from_str(&sample_config());
        let mut cfg = SpicetifyConfig::load(&path).unwrap();
        assert!(cfg.list_add("custom_apps", "lyrics-plus"));
        assert!(!cfg.list_add("custom_apps", "lyrics-plus"));
        assert_eq!(cfg.custom_apps(), ["new-releases", "reddit", "lyrics-plus"]);
    }

    #[test]
    fn list_remove_exact_match_only() {
        let (_dir, path) = load_from_str(&sample_config());
        let mut cfg = SpicetifyConfig::load(&path).unwrap();
        assert!(!cfg.list_remove("custom_apps", "releases"));
        assert!(cfg.list_remove("custom_apps", "reddit"));
        assert_eq!(cfg.custom_apps(), ["new-releases"]);
    }

    #[test]
    fn roundtrip_preserves_other_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config-xpui.ini");
        std::fs::write(&path, sample_config()).unwrap();

        let mut cfg = SpicetifyConfig::load(&path).unwrap();
        cfg.set_string(SETTING, "current_theme", "Comfy");
        cfg.list_add("extensions", "myext.js");
        cfg.save().unwrap();

        let reloaded = SpicetifyConfig::load(&path).unwrap();
        assert_eq!(reloaded.current_theme().as_deref(), Some("Comfy"));
        assert_eq!(reloaded.extensions(), ["myext.js"]);
        assert_eq!(
            reloaded
                .get_string("Preprocesses", "disable_sentry")
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            reloaded.custom_apps(),
            ["new-releases", "reddit"],
            "untouched lists stay intact"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[Setting]"), "sections survive");
        assert!(!text.contains("spicepm-tmp"), "no temp files left behind");
    }
}
