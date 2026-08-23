//! Fetching CSS snippets, ported from `fetchCssSnippets`.

use super::constants::{MARKETPLACE_RAW_BASE, SNIPPETS_TTL_SECS, SNIPPETS_URL};
use super::types::Snippet;
use crate::errors::Result;
use crate::http::HttpClient;

/// Fetch all snippets and resolve relative previews to absolute URLs
/// (`https://raw.githubusercontent.com/spicetify/marketplace/main/{preview}`).
pub async fn fetch_snippets(http: &HttpClient) -> Result<Vec<Snippet>> {
    let mut snippets: Vec<Snippet> =
        http.get_json_cached(SNIPPETS_URL, SNIPPETS_TTL_SECS).await?;

    for snip in &mut snippets {
        if let Some(preview) = snip.preview.take() {
            snip.image_url = Some(if preview.starts_with("http") {
                preview
            } else {
                format!("{MARKETPLACE_RAW_BASE}/{preview}")
            });
        }
    }
    Ok(snippets)
}
