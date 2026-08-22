//! TTL disk cache for HTTP responses, stored in the OS cache dir:
//! Linux `$XDG_CACHE_HOME/spicepm`, macOS `~/Library/Caches/spicepm`,
//! Windows `%LOCALAPPDATA%\spicepm\cache`.
//!
//! Layout: `<dir>/<sha256(url)>.body` + `<sha256(url)>.meta.json`
//! with `{ url, fetched_at, ttl }` metadata.

use crate::errors::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    url: String,
    fetched_at: u64,
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new() -> Result<Self> {
        Ok(Self { dir: cache_dir()? })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get_fresh(&self, url: &str, ttl_secs: u64) -> Option<Vec<u8>> {
        let key = hash_key(url);
        let meta_path = self.dir.join(format!("{key}.meta.json"));
        let body_path = self.dir.join(format!("{key}.body"));

        let meta_raw = std::fs::read(meta_path).ok()?;
        let meta: Meta = serde_json::from_slice(&meta_raw).ok()?;
        if meta.url != url {
            return None;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        if now.saturating_sub(meta.fetched_at) >= ttl_secs {
            return None;
        }
        std::fs::read(body_path).ok()
    }

    pub fn store(&self, url: &str, body: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let key = hash_key(url);
        let meta = Meta {
            url: url.to_owned(),
            fetched_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
        };
        let meta_path = self.dir.join(format!("{key}.meta.json"));
        let body_path = self.dir.join(format!("{key}.body"));
        atomic_write(&meta_path, serde_json::to_vec(&meta)?.as_slice())?;
        atomic_write(&body_path, body)?;
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)?;
        }
        Ok(())
    }
}

fn hash_key(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let hex: String = digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    hex[..32].to_owned()
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::other(format!("no parent dir for {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension("spicepm-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn cache_dir() -> Result<PathBuf> {
    if let Ok(v) = std::env::var("SPICEPM_CACHE")
        && !v.is_empty()
    {
        return Ok(PathBuf::from(v));
    }
    if cfg!(windows) {
        let local =
            std::env::var("LOCALAPPDATA").map_err(|_| Error::other("LOCALAPPDATA not set"))?;
        return Ok(PathBuf::from(local).join("spicepm").join("cache"));
    }
    if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").map_err(|_| Error::other("HOME not set"))?;
        return Ok(PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("spicepm"));
    }
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let home = std::env::var("HOME").map_err(|_| Error::other("HOME not set"))?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(base.join("spicepm"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache {
            dir: dir.path().to_path_buf(),
        };
        cache.store("https://example.com/a", b"hello").unwrap();
        assert_eq!(
            cache.get_fresh("https://example.com/a", 60).unwrap(),
            b"hello"
        );
        assert!(cache.get_fresh("https://example.com/b", 60).is_none());
        // expired
        assert!(cache.get_fresh("https://example.com/a", 0).is_none());
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("f.txt");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
    }
}
