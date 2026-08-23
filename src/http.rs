//! HTTP client wrapper: consistent User-Agent, optional GitHub token,
//! rate-limit detection, retries, and a TTL disk cache.

use crate::cache::Cache;
use crate::errors::{Error, Result};
use crate::ui;
use std::time::Duration;

pub struct HttpClient {
    inner: reqwest::Client,
    token: Option<String>,
    cache: Cache,
    no_cache: bool,
}

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(800);

/// Result of a (possibly conditional) GET.
enum Fetch {
    Body(Vec<u8>, Option<String>),
    NotModified,
}

impl HttpClient {
    pub fn new(no_cache: bool) -> Result<Self> {
        Ok(Self {
            inner: Self::build_client()?,
            token: github_token(),
            cache: Cache::new()?,
            no_cache,
        })
    }

    #[cfg(test)]
    fn new_with_cache(no_cache: bool, dir: std::path::PathBuf) -> Result<Self> {
        Ok(Self {
            inner: Self::build_client()?,
            token: github_token(),
            cache: Cache::at(dir),
            no_cache,
        })
    }

    fn build_client() -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder()
            .user_agent(concat!("spicepm/", env!("CARGO_PKG_VERSION")))
            .gzip(true)
            .build()?)
    }

    fn request(&self, url: &str, api: bool) -> reqwest::RequestBuilder {
        let mut builder = self.inner.get(url);
        if api
            && url.starts_with("https://api.github.com")
            && let Some(token) = &self.token
        {
            builder = builder.bearer_auth(token);
        }
        builder
    }

    async fn fetch(&self, url: &str, if_none_match: Option<&str>) -> Result<Fetch> {
        let mut attempt = 0u32;
        loop {
            let mut builder = self.request(url, true);
            if let Some(etag) = if_none_match {
                builder = builder.header(reqwest::header::IF_NONE_MATCH, etag);
            }
            let send_result = builder.send().await;
            let response = match send_result {
                Ok(response) => response,
                Err(err) => {
                    // connection-level failures (reset/timeout/DNS) are
                    // always worth retrying
                    if err.status().is_none() && attempt < MAX_RETRIES {
                        tracing::debug!(url, "network error {err}; retrying");
                        tokio::time::sleep(RETRY_DELAY * (attempt + 1)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(err.into());
                }
            };
            let status = response.status();

            if status.as_u16() == 403 || status.as_u16() == 429 {
                let remaining = response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                // only an explicitly exhausted quota (or a bare 429) is a
                // rate limit; any other 403 is a plain refusal (private
                // repo, blocked path, ...) and must not be misreported
                if status.as_u16() == 429 || remaining.as_deref() == Some("0") {
                    let reset = response
                        .headers()
                        .get("x-ratelimit-reset")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map_or_else(|| "later".to_owned(), unix_to_readable);
                    return Err(Error::RateLimited {
                        status: status.as_u16(),
                        reset,
                    });
                }
                // 403 without exhausted quota: treat like a transient failure
            }

            if status.as_u16() == 304 {
                return Ok(Fetch::NotModified);
            }

            if status.as_u16() == 404 {
                // common cause: manifest lists a file missing upstream
                return Err(Error::other(format!(
                    "file not found upstream (404): {url}"
                )));
            }

            match response.error_for_status() {
                Ok(response) => {
                    let etag = response
                        .headers()
                        .get(reqwest::header::ETAG)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    let bytes_result = response.bytes().await;
                    match bytes_result {
                        Ok(bytes) => return Ok(Fetch::Body(bytes.to_vec(), etag)),
                        Err(err) => {
                            if attempt < MAX_RETRIES {
                                tracing::debug!(url, "body read error {err}; retrying");
                                tokio::time::sleep(RETRY_DELAY * (attempt + 1)).await;
                                attempt += 1;
                                continue;
                            }
                            return Err(err.into());
                        }
                    }
                }
                Err(err) => {
                    let retryable = err.status().is_some_and(|s| s.is_server_error());
                    if retryable && attempt < MAX_RETRIES {
                        tracing::debug!(url, "retryable error {err}; retrying");
                        tokio::time::sleep(RETRY_DELAY * (attempt + 1)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(err.into());
                }
            }
        }
    }

    /// Fetch JSON, bypassing the cache entirely (search endpoints etc).
    pub async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        let bytes = self.fetch(url, None).await?;
        let Fetch::Body(bytes, _) = bytes else {
            return Err(Error::other("unexpected 304 without a conditional request"));
        };
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Fetch JSON through the TTL cache. The cache decides how to proceed
    /// ([`Cache::plan`]): serve fresh bytes, or revalidate with the stored
    /// `ETag` on expiry (`304` responses cost no rate-limit quota). When a
    /// refresh fails outright, a stale copy is served as a last resort.
    pub async fn get_json_cached<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        ttl_secs: u64,
    ) -> Result<T> {
        match self.cache.plan(url, ttl_secs, !self.no_cache) {
            crate::cache::Plan::Serve(bytes) => {
                tracing::debug!(url, "cache hit");
                Ok(serde_json::from_slice(&bytes)?)
            }
            crate::cache::Plan::Fetch { etag } => self.fetch_and_cache(url, etag.as_deref()).await,
        }
    }

    /// Network leg of [`Self::get_json_cached`]: fetch (conditionally),
    /// commit the result to the cache, or fall back to a stale copy.
    async fn fetch_and_cache<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        etag: Option<&str>,
    ) -> Result<T> {
        match self.fetch(url, etag).await {
            Ok(Fetch::Body(bytes, new_etag)) => {
                self.cache.store(url, &bytes, new_etag.as_deref())?;
                Ok(serde_json::from_slice(&bytes)?)
            }
            Ok(Fetch::NotModified) => {
                self.cache.touch(url)?;
                tracing::debug!(url, "304 not modified; cache refreshed for free");
                let bytes = self
                    .cache
                    .get_stale(url)
                    .ok_or_else(|| Error::other("server answered 304 but no cached body exists"))?;
                Ok(serde_json::from_slice(&bytes)?)
            }
            Err(err) => {
                // --no-cache users explicitly asked for fresh data
                if self.no_cache {
                    return Err(err);
                }
                match self.cache.stale_with_age(url) {
                    Some((bytes, age)) => {
                        tracing::warn!(url, "refresh failed ({err}); serving stale copy");
                        ui::warn(format!(
                            "couldn't refresh - serving {} cached copy ({err})",
                            human_age(age)
                        ));
                        Ok(serde_json::from_slice(&bytes)?)
                    }
                    None => Err(err),
                }
            }
        }
    }

    /// Fetch arbitrary bytes (files to install), always fresh.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let fetched = self.fetch(url, None).await?;
        let Fetch::Body(bytes, _) = fetched else {
            return Err(Error::other("unexpected 304 without a conditional request"));
        };
        Ok(bytes)
    }
}

/// Humanize a staleness duration for warning messages.
fn human_age(secs: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    match secs {
        0..=MINUTE => "a moment".to_owned(),
        s if s < HOUR => format!("{}-minute-old", s / MINUTE),
        s if s < DAY => format!("{}-hour-old", s / HOUR),
        s => format!("{}-day-old", s / DAY),
    }
}

fn github_token() -> Option<String> {
    for key in ["SPICEPM_GITHUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(value) = std::env::var(key)
            && !value.trim().is_empty()
        {
            tracing::debug!(key, "using GitHub token from environment");
            return Some(value);
        }
    }
    None
}

fn unix_to_readable(secs: u64) -> String {
    // RFC-3339-ish UTC formatting without pulling chrono in
    const SECS_PER_DAY: u64 = 86_400;
    let days = i64::try_from(secs / SECS_PER_DAY).unwrap_or(i64::from(i16::MAX));
    let rem = secs % SECS_PER_DAY;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

// Howard Hinnant's civil_from_days
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_time() {
        assert_eq!(unix_to_readable(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_to_readable(1_755_000_000), "2025-08-12T12:00:00Z");
        assert_eq!(unix_to_readable(86_400), "1970-01-02T00:00:00Z");
    }

    #[test]
    fn no_token_when_env_unset() {
        // can't guarantee CI env; just ensure function doesn't panic and type matches
        let _ = github_token();
    }

    #[test]
    fn humanizes_staleness() {
        assert_eq!(human_age(30), "a moment");
        assert_eq!(human_age(300), "5-minute-old");
        assert_eq!(human_age(7_200), "2-hour-old");
        assert_eq!(human_age(3 * 86_400), "3-day-old");
    }

    /// Wiremock-backed checks for the cached fetch flow. These live in the
    /// unit test module so they can reach into the private `cache` field.
    mod net {
        use super::*;
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const URL_PATH: &str = "/cached.json";

        async fn server_for() -> (MockServer, String, tempfile::TempDir) {
            let server = MockServer::start().await;
            let url = format!("{}{URL_PATH}", server.uri());
            let dir = tempfile::tempdir().unwrap();
            (server, url, dir)
        }

        #[tokio::test]
        async fn revalidates_with_etag_and_serves_304() {
            let (server, url, dir) = server_for().await;
            let client = HttpClient::new_with_cache(false, dir.path().to_path_buf()).unwrap();
            client.cache.store(&url, b"\"v1\"", Some("\"e1\"")).unwrap();

            // only matches when the conditional header is sent; a missing
            // header would 404 and fail the assertion below instead.
            // Expectations are checked automatically when the guard drops.
            let _guard = Mock::given(method("GET"))
                .and(path(URL_PATH))
                .and(header("if-none-match", "\"e1\""))
                .respond_with(ResponseTemplate::new(304))
                .expect(1)
                .mount_as_scoped(&server)
                .await;

            let value: serde_json::Value = client.get_json_cached(&url, 0).await.unwrap();
            assert_eq!(value, serde_json::json!("v1"));
        }

        #[tokio::test]
        async fn stores_fresh_etag_on_200() {
            let (server, url, dir) = server_for().await;
            let client = HttpClient::new_with_cache(false, dir.path().to_path_buf()).unwrap();

            Mock::given(method("GET"))
                .and(path(URL_PATH))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string("\"fresh\"")
                        .insert_header("etag", "\"e2\""),
                )
                .mount(&server)
                .await;

            let value: serde_json::Value = client.get_json_cached(&url, 0).await.unwrap();
            assert_eq!(value, serde_json::json!("fresh"));
            assert_eq!(client.cache.stored_etag(&url).unwrap(), "\"e2\"");
            assert_eq!(
                client.cache.get_fresh(&url, 60).unwrap(),
                b"\"fresh\"".to_vec()
            );
        }

        #[tokio::test]
        async fn serves_stale_when_rate_limited() {
            let (server, url, dir) = server_for().await;
            let client = HttpClient::new_with_cache(false, dir.path().to_path_buf()).unwrap();
            client.cache.store(&url, b"\"stale\"", None).unwrap();

            // expired entry + rate-limited refresh -> stale copy served,
            // and exactly one request was made (no retry storm); the
            // scoped guard's drop verifies the expectation count
            let _guard = Mock::given(method("GET"))
                .and(path(URL_PATH))
                .respond_with(
                    ResponseTemplate::new(403)
                        .insert_header("x-ratelimit-remaining", "0")
                        .insert_header("x-ratelimit-reset", "4102444800"),
                )
                .expect(1)
                .mount_as_scoped(&server)
                .await;

            let value: serde_json::Value = client.get_json_cached(&url, 0).await.unwrap();
            assert_eq!(value, serde_json::json!("stale"));
        }

        #[tokio::test]
        async fn plain_403_is_not_reported_as_rate_limit() {
            let (server, url, dir) = server_for().await;
            // no_cache so the error propagates instead of falling back to stale
            let client = HttpClient::new_with_cache(true, dir.path().to_path_buf()).unwrap();

            let _guard = Mock::given(method("GET"))
                .and(path(URL_PATH))
                .respond_with(ResponseTemplate::new(403)) // no ratelimit headers
                .expect(1) // must not be retried as transient either
                .mount_as_scoped(&server)
                .await;

            let result: Result<serde_json::Value> = client.get_json_cached(&url, 0).await;
            assert!(
                !matches!(result, Err(Error::RateLimited { .. })),
                "plain 403 misreported as rate limit: {result:?}"
            );
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn no_cache_flag_refuses_stale_fallback() {
            let (server, url, dir) = server_for().await;
            let client = HttpClient::new_with_cache(true, dir.path().to_path_buf()).unwrap();
            client.cache.store(&url, b"\"stale\"", None).unwrap();

            Mock::given(method("GET"))
                .and(path(URL_PATH))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;

            let result: Result<serde_json::Value> = client.get_json_cached(&url, 0).await;
            assert!(result.is_err(), "--no-cache must not serve stale data");
        }
    }
}
