//! Spicetify directory layout, a faithful port of the CLI's
//! `GetSpicetifyFolder` / `GetStateFolder`:
//!
//! | OS      | config dir                                   |
//! |---------|----------------------------------------------|
//! | Windows | `%APPDATA%\spicetify`                        |
//! | Linux   | `$XDG_CONFIG_HOME/spicetify` or `~/.config/spicetify` |
//! | macOS   | `~/.config/spicetify`                        |
//!
//! `SPICETIFY_CONFIG` / `SPICETIFY_STATE` env vars override everything.

use crate::errors::{Error, Result};
use std::path::PathBuf;

/// Env-lookup abstraction so resolution is testable without mutating
/// process-global state.
type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn real_env() -> impl Fn(&str) -> Option<String> {
    |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty())
}

pub fn spicetify_dir() -> PathBuf {
    spicetify_dir_with(&real_env())
}

fn spicetify_dir_with(env: EnvLookup) -> PathBuf {
    // the Go source treats set-but-empty as unset (`len(result) > 0`)
    if let Some(v) = env("SPICETIFY_CONFIG").filter(|v| !v.is_empty()) {
        return PathBuf::from(v);
    }
    if cfg!(windows) {
        let appdata = env("APPDATA").unwrap_or_default();
        return PathBuf::from(appdata).join("spicetify");
    }
    // linux and darwin both use ~/.config (matching the Go source)
    let base = match env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => home_dir_from(env).join(".config"),
    };
    base.join("spicetify")
}

/// State dir port (Backup/Extracted home); reserved for future milestones.
#[expect(dead_code)]
pub fn state_dir() -> PathBuf {
    state_dir_with(&real_env())
}

#[cfg_attr(not(test), expect(dead_code))]
fn state_dir_with(env: EnvLookup) -> PathBuf {
    if let Some(v) = env("SPICETIFY_STATE").filter(|v| !v.is_empty()) {
        return PathBuf::from(v);
    }
    if cfg!(windows) {
        let appdata = env("APPDATA").unwrap_or_default();
        return PathBuf::from(appdata).join("spicetify");
    }
    let base = match env("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => home_dir_from(env).join(".local").join("state"),
    };
    base.join("spicetify")
}

pub fn extensions_dir() -> PathBuf {
    spicetify_dir().join("Extensions")
}

pub fn themes_dir() -> PathBuf {
    spicetify_dir().join("Themes")
}

/// Custom apps arrive in a later milestone (M6).
#[expect(dead_code)]
pub fn custom_apps_dir() -> PathBuf {
    spicetify_dir().join("CustomApps")
}

pub fn config_file() -> PathBuf {
    spicetify_dir().join("config-xpui.ini")
}

pub fn spicepm_data_dir() -> PathBuf {
    spicetify_dir().join("spicepm")
}

/// Verify the spicetify config dir exists; return a helpful error otherwise.
pub fn require_spicetify_dir() -> Result<PathBuf> {
    let dir = spicetify_dir();
    if !dir.is_dir() {
        return Err(Error::SpicetifyDirNotFound {
            dir: dir.display().to_string(),
        });
    }
    Ok(dir)
}

fn home_dir_from(env: EnvLookup) -> PathBuf {
    env("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| env("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn env_override_wins() {
        let env = fake_env(&[("SPICETIFY_CONFIG", "/tmp/cfg")]);
        assert_eq!(spicetify_dir_with(&env), PathBuf::from("/tmp/cfg"));
        assert_eq!(
            extensions_dir_for(&env),
            PathBuf::from("/tmp/cfg/Extensions")
        );
        assert_eq!(themes_dir_for(&env), PathBuf::from("/tmp/cfg/Themes"));
        assert_eq!(
            config_file_for(&env),
            PathBuf::from("/tmp/cfg/config-xpui.ini")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn linux_uses_xdg_then_home() {
        let xdg = fake_env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]);
        assert_eq!(spicetify_dir_with(&xdg), PathBuf::from("/xdg/spicetify"));

        let home_only = fake_env(&[("HOME", "/home/u")]);
        assert_eq!(
            spicetify_dir_with(&home_only),
            PathBuf::from("/home/u/.config/spicetify")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn empty_env_values_are_ignored_like_the_go_source() {
        let env = fake_env(&[
            ("SPICETIFY_CONFIG", ""),
            ("XDG_CONFIG_HOME", ""),
            ("HOME", "/h"),
        ]);
        assert_eq!(
            spicetify_dir_with(&env),
            PathBuf::from("/h/.config/spicetify")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn state_dir_prefers_xdg_state_home() {
        let env = fake_env(&[("XDG_STATE_HOME", "/state"), ("HOME", "/h")]);
        assert_eq!(state_dir_with(&env), PathBuf::from("/state/spicetify"));
        let home_only = fake_env(&[("HOME", "/h")]);
        assert_eq!(
            state_dir_with(&home_only),
            PathBuf::from("/h/.local/state/spicetify")
        );
    }

    // helpers mirroring the public path fns but against injected env
    fn extensions_dir_for(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
        spicetify_dir_with(&env).join("Extensions")
    }
    fn themes_dir_for(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
        spicetify_dir_with(&env).join("Themes")
    }
    fn config_file_for(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
        spicetify_dir_with(&env).join("config-xpui.ini")
    }

    #[test]
    #[cfg(windows)]
    fn windows_uses_appdata() {
        let appdata = "C:/Users/u/AppData/Roaming";
        let pairs = [("APPDATA", appdata)];
        let env = fake_env(&pairs);
        assert_eq!(
            spicetify_dir_with(&env),
            PathBuf::from(appdata).join("spicetify")
        );
        assert_eq!(
            state_dir_with(&env),
            PathBuf::from(appdata).join("spicetify")
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_missing_appdata_is_relative_fallback() {
        let env = fake_env(&[]);
        assert_eq!(
            spicetify_dir_with(&env),
            PathBuf::from("").join("spicetify")
        );
        assert_eq!(state_dir_with(&env), PathBuf::from("").join("spicetify"));
    }
}
