//! TTL disk cache for HTTP responses, stored in the OS cache dir:
//! Linux `$XDG_CACHE_HOME/spicepm`, macOS `~/Library/Caches/spicepm`,
//! Windows `%LOCALAPPDATA%\spicepm\cache`.
//!
//! Layout: `<dir>/<sha256(url)>.body` + `<dir>/<sha256(url)>.meta.json`
//! with `{ url, fetched_at, etag }` metadata. The body is always written
//! before the metadata, so an interrupted store can only ever look expired -
//! never falsely fresh. Startup sweeps also drop expired entries, corrupt
//! metadata, orphaned bodies and abandoned temp files.

use crate::errors::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Entries older than this are swept on startup (best effort).
const PRUNE_AFTER_SECS: u64 = 30 * 24 * 60 * 60;

/// Marker inside `atomic_write` temp file names; startup sweeps remove any
/// leftover bearing it.
const TMP_SUFFIX: &str = ".spicepm-tmp-";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize)]
struct Meta {
    url: String,
    fetched_at: u64,
    #[serde(default)]
    etag: Option<String>,
}

/// What `stats` counted: number of cached bodies and their total size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: u64,
}

/// Next step for a cached-eligible GET, decided by [`Cache::plan`].
#[derive(Debug)]
pub enum Plan {
    /// Fresh enough - serve these bytes without touching the network.
    Serve(Vec<u8>),
    /// Go to the network; send `etag` as `If-None-Match` when present.
    Fetch { etag: Option<String> },
}

pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new() -> Result<Self> {
        Ok(Self::at(cache_dir()?))
    }

    /// Infallible constructor for tests and explicit locations.
    pub fn at(dir: PathBuf) -> Self {
        let cache = Self { dir };
        cache.prune_stale();
        cache
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get_fresh(&self, url: &str, ttl_secs: u64) -> Option<Vec<u8>> {
        let meta = self.read_meta(url)?;
        if age_secs(meta.fetched_at) >= ttl_secs {
            return None;
        }
        self.read_body(url)
    }

    /// Cached bytes regardless of age - the last-resort fallback when a
    /// refresh fails (rate limit, offline).
    pub fn get_stale(&self, url: &str) -> Option<Vec<u8>> {
        self.read_meta(url)?;
        self.read_body(url)
    }

    pub fn stored_etag(&self, url: &str) -> Option<String> {
        self.read_meta(url)?.etag
    }

    /// Seconds since the entry was fetched (for "serving N-minute-old copy"
    /// style messages).
    pub fn age_secs(&self, url: &str) -> Option<u64> {
        Some(age_secs(self.read_meta(url)?.fetched_at))
    }

    /// Decide how a cached-eligible GET for `url` should proceed: serve a
    /// fresh body, or hit the network - conditionally when an `ETag` is on
    /// file (`allow_cache == false` means "--no-cache": always unconditional).
    pub fn plan(&self, url: &str, ttl_secs: u64, allow_cache: bool) -> Plan {
        if !allow_cache {
            return Plan::Fetch { etag: None };
        }
        if let Some(bytes) = self.get_fresh(url, ttl_secs) {
            return Plan::Serve(bytes);
        }
        Plan::Fetch {
            etag: self.stored_etag(url),
        }
    }

    /// The stale body plus its age, for the last-resort fallback messaging.
    pub fn stale_with_age(&self, url: &str) -> Option<(Vec<u8>, u64)> {
        let bytes = self.get_stale(url)?;
        Some((bytes, self.age_secs(url).unwrap_or_default()))
    }

    /// Restart the freshness clock without refetching (304 Not Modified).
    pub fn touch(&self, url: &str) -> Result<()> {
        let mut meta = self
            .read_meta(url)
            .ok_or_else(|| Error::other(format!("nothing cached for {url}")))?;
        meta.fetched_at = now_secs();
        self.write_meta(&meta)
    }

    pub fn store(&self, url: &str, body: &[u8], etag: Option<&str>) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        // body first: the metadata write is the commit marker
        atomic_write(&self.body_path(url), body)?;
        self.write_meta(&Meta {
            url: url.to_owned(),
            fetched_at: now_secs(),
            etag: etag.map(str::to_owned),
        })
    }

    /// (entry count, total body bytes)
    pub fn stats(&self) -> CacheStats {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return CacheStats::default();
        };
        let mut stats = CacheStats::default();
        for entry in entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".body"))
        {
            stats.entries += 1;
            stats.bytes += entry.metadata().map_or(0, |m| m.len());
        }
        stats
    }

    /// Best-effort sweep of entries past their prune window; corrupt
    /// metadata is treated as garbage and removed too, as are orphaned
    /// bodies (an interrupted `store`) and abandoned `atomic_write`
    /// temporaries.
    fn prune_stale(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.contains(TMP_SUFFIX) {
                // leftover of an interrupted atomic_write
                let _ = std::fs::remove_file(entry.path());
                continue;
            }
            if let Some(stem) = name.strip_suffix(".body") {
                // orphan body: the metadata commit never happened
                let meta_path = self.dir.join(format!("{stem}.meta.json"));
                if !meta_path.exists() {
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            let Some(stem) = name.strip_suffix(".meta.json") else {
                continue;
            };
            let body_path = self.dir.join(format!("{stem}.body"));
            let meta = std::fs::read(entry.path())
                .ok()
                .and_then(|raw| serde_json::from_slice::<Meta>(&raw).ok());
            let expired = match meta {
                Some(meta) => age_secs(meta.fetched_at) > PRUNE_AFTER_SECS,
                None => true, // unreadable/corrupt
            };
            if expired {
                let _ = std::fs::remove_file(entry.path());
                let _ = std::fs::remove_file(body_path);
            }
        }
    }

    fn read_meta(&self, url: &str) -> Option<Meta> {
        let raw = std::fs::read(self.meta_path(url)).ok()?;
        let meta: Meta = serde_json::from_slice(&raw).ok()?;
        (meta.url == url).then_some(meta)
    }

    fn read_body(&self, url: &str) -> Option<Vec<u8>> {
        std::fs::read(self.body_path(url)).ok()
    }

    fn write_meta(&self, meta: &Meta) -> Result<()> {
        atomic_write(
            &self.meta_path(&meta.url),
            serde_json::to_vec(meta)?.as_slice(),
        )
    }

    fn meta_path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{}.meta.json", hash_key(url)))
    }

    fn body_path(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{}.body", hash_key(url)))
    }
}

pub fn clear_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn age_secs(fetched_at: u64) -> u64 {
    now_secs().saturating_sub(fetched_at)
}

/// Truncated sha256 of the URL; 128 bits make collisions a non-issue.
fn hash_key(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let hex: String = digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    hex[..32].to_owned()
}

/// Write via a unique temp file + rename, so readers never observe partial
/// content and concurrent writers cannot clobber each other's temp file.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::other(format!("no parent dir for {}", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!("{TMP_SUFFIX}{}-{seq}", std::process::id()));
    let tmp = parent.join(tmp_name);
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

    fn test_cache() -> (Cache, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Cache::at(dir.path().to_path_buf()), dir)
    }

    #[test]
    fn roundtrip_and_expiry() {
        let (cache, _dir) = test_cache();
        cache
            .store("https://example.com/a", b"hello", None)
            .unwrap();
        assert_eq!(
            cache.get_fresh("https://example.com/a", 60).unwrap(),
            b"hello"
        );
        assert!(cache.get_fresh("https://example.com/b", 60).is_none());
        // expired for get_fresh, still available as stale
        assert!(cache.get_fresh("https://example.com/a", 0).is_none());
        assert_eq!(cache.get_stale("https://example.com/a").unwrap(), b"hello");
    }

    #[test]
    fn corrupted_meta_is_ignored() {
        let (cache, dir) = test_cache();
        cache.store("https://example.com/a", b"body", None).unwrap();
        let key = hash_key("https://example.com/a");
        std::fs::write(dir.path().join(format!("{key}.meta.json")), b"{ not json").unwrap();
        assert!(cache.get_fresh("https://example.com/a", 60).is_none());
        assert!(cache.get_stale("https://example.com/a").is_none());
    }

    #[test]
    fn meta_url_mismatch_is_ignored() {
        // simulates a hash-key collision between two URLs
        let (cache, dir) = test_cache();
        cache
            .store("https://example.com/real", b"body", None)
            .unwrap();
        let key = hash_key("https://example.com/real");
        let meta_path = dir.path().join(format!("{key}.meta.json"));
        let mut meta: Meta = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta.url = "https://example.com/other".to_owned();
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
        assert!(cache.get_fresh("https://example.com/real", 60).is_none());
        assert!(cache.get_stale("https://example.com/real").is_none());
    }

    #[test]
    fn etag_roundtrip_and_touch() {
        let (cache, dir) = test_cache();
        assert!(cache.stored_etag("https://example.com/e").is_none());
        cache
            .store("https://example.com/e", b"v1", Some("\"abc\""))
            .unwrap();
        assert_eq!(
            cache.stored_etag("https://example.com/e").unwrap(),
            "\"abc\""
        );
        // touch restarts the clock: ttl=0 means expired, so fresh-after-touch
        // is observable by bumping fetched_at far into the past first
        let key = hash_key("https://example.com/e");
        let meta_path = dir.path().join(format!("{key}.meta.json"));
        let mut meta: Meta = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta.fetched_at = now_secs().saturating_sub(10_000);
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
        assert!(cache.get_fresh("https://example.com/e", 60).is_none());

        cache.touch("https://example.com/e").unwrap();
        assert!(cache.get_fresh("https://example.com/e", 60).is_some());
    }

    #[test]
    fn stats_count_bodies_only() {
        let (cache, _dir) = test_cache();
        assert_eq!(cache.stats(), CacheStats::default());
        cache
            .store("https://example.com/s1", b"12345", None)
            .unwrap();
        cache.store("https://example.com/s2", b"12", None).unwrap();
        assert_eq!(
            cache.stats(),
            CacheStats {
                entries: 2,
                bytes: 7
            }
        );
    }

    #[test]
    fn prune_drops_old_and_corrupt_entries() {
        let (cache, dir) = test_cache();
        cache
            .store("https://example.com/old", b"data", None)
            .unwrap();
        let key = hash_key("https://example.com/old");
        let meta_path = dir.path().join(format!("{key}.meta.json"));
        let mut meta: Meta = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta.fetched_at = now_secs().saturating_sub(PRUNE_AFTER_SECS + 1);
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();

        // a corrupt entry
        cache
            .store("https://example.com/bad", b"data", None)
            .unwrap();
        let bad_key = hash_key("https://example.com/bad");
        std::fs::write(dir.path().join(format!("{bad_key}.meta.json")), b"junk").unwrap();

        cache.prune_stale();

        assert!(!meta_path.exists());
        assert!(!dir.path().join(format!("{key}.body")).exists());
        assert!(!dir.path().join(format!("{bad_key}.meta.json")).exists());
        assert!(!dir.path().join(format!("{bad_key}.body")).exists());
    }

    #[test]
    fn prune_sweeps_orphan_bodies_and_temp_files_but_keeps_live_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // interrupted store: body committed, meta never written
        let key = hash_key("https://example.com/orphan");
        std::fs::write(path.join(format!("{key}.body")), b"orphan").unwrap();
        // abandoned atomic_write temp
        std::fs::write(path.join(format!("{key}.body{TMP_SUFFIX}42-0")), b"junk").unwrap();

        // a live entry must survive the same sweep
        let warm = Cache::at(path.to_path_buf());
        warm.store("https://example.com/live", b"kept", None)
            .unwrap();

        Cache::at(path.to_path_buf()); // constructor runs the sweep

        assert!(!path.join(format!("{key}.body")).exists());
        assert!(!path.join(format!("{key}.body{TMP_SUFFIX}42-0")).exists());
        let live = hash_key("https://example.com/live");
        assert!(path.join(format!("{live}.body")).exists());
        assert!(path.join(format!("{live}.meta.json")).exists());
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("f.txt");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        // no leftover temp files
        assert_eq!(
            std::fs::read_dir(dir.path().join("sub")).unwrap().count(),
            1
        );
    }
}
