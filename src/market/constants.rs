/// Max GitHub API items per page.
pub const ITEMS_PER_REQUEST: u32 = 100;

pub const SNIPPETS_URL: &str =
    "https://raw.githubusercontent.com/spicetify/marketplace/main/resources/snippets.json";

pub const BLACKLIST_URL: &str =
    "https://raw.githubusercontent.com/spicetify/marketplace/main/resources/blacklist.json";

pub const MARKETPLACE_RAW_BASE: &str =
    "https://raw.githubusercontent.com/spicetify/marketplace/main";

// Used for the future `spicepm self-update` check (marketplace parity).
#[expect(dead_code)]
pub const LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/spicetify/marketplace/releases/latest";

const GITHUB_SEARCH_URL: &str = "https://api.github.com/search/repositories";

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum RepoTopic {
    Extensions,
    Themes,
    Apps,
}

impl RepoTopic {
    pub fn as_str(self) -> &'static str {
        match self {
            RepoTopic::Extensions => "spicetify-extensions",
            RepoTopic::Themes => "spicetify-themes",
            RepoTopic::Apps => "spicetify-apps",
        }
    }
}

pub fn search_repos_url(tag: &str, page: u32) -> String {
    let q = urlencode(tag);
    format!("{GITHUB_SEARCH_URL}?q=topic:{q}&per_page={ITEMS_PER_REQUEST}&page={page}")
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0F) as usize] as char);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_url_matches_marketplace_format() {
        let url = search_repos_url("spicetify-extensions", 1);
        assert_eq!(
            url,
            "https://api.github.com/search/repositories?q=topic:spicetify-extensions&per_page=100&page=1".to_string()
        );
    }

    #[test]
    fn urlencode_escapes() {
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }
}
