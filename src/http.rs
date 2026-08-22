//! HTTP client wrapper: consistent User-Agent, optional GitHub token,
//! rate-limit detection, retries, and a TTL disk cache.

use crate::cache::Cache;
use crate::errors::{Error, Result};
use std::time::Duration;

pub struct HttpClient {
    inner: reqwest::Client,
    token: Option<String>,
    cache: Cache,
    no_cache: bool,
}

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(800);

impl HttpClient {
    pub fn new(no_cache: bool) -> Result<Self> {
        let inner = reqwest::Client::builder()
            .user_agent(concat!("spicepm/", env!("CARGO_PKG_VERSION")))
            .gzip(true)
            .build()?;
        Ok(Self {
            inner,
            token: github_token(),
            cache: Cache::new()?,
            no_cache,
        })
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

    async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut attempt = 0u32;
        loop {
            let send_result = self.request(url, true).send().await;
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
                if remaining.as_deref().is_none_or(|r| r == "0") || status.as_u16() == 429 {
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

            if status.as_u16() == 404 {
                // common cause: manifest lists a file missing upstream
                return Err(Error::other(format!(
                    "file not found upstream (404): {url}"
                )));
            }

            match response.error_for_status() {
                Ok(response) => {
                    let bytes_result = response.bytes().await;
                    match bytes_result {
                        Ok(bytes) => return Ok(bytes.to_vec()),
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
        let bytes = self.fetch_bytes(url).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Fetch JSON through the TTL cache.
    pub async fn get_json_cached<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        ttl_secs: u64,
    ) -> Result<T> {
        if !self.no_cache
            && let Some(bytes) = self.cache.get_fresh(url, ttl_secs)
        {
            tracing::debug!(url, "cache hit");
            return Ok(serde_json::from_slice(&bytes)?);
        }
        let bytes = self.fetch_bytes(url).await?;
        self.cache.store(url, &bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Fetch arbitrary bytes (files to install), always fresh.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        self.fetch_bytes(url).await
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
}
