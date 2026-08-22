use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("config parse error: {0}")]
    Ini(#[from] ini::Error),
    #[error(
        "rate limited by the GitHub API (HTTP {status}). Wait until {reset} or set GITHUB_TOKEN to raise the limit"
    )]
    RateLimited { status: u16, reset: String },
    #[error(
        "spicetify config directory not found at `{dir}`. Is spicetify installed? Set SPICETIFY_CONFIG to override"
    )]
    SpicetifyDirNotFound { dir: String },
    #[error("spicetify config file not found at `{path}`. Run `spicetify` once to generate it")]
    SpicetifyConfigNotFound { path: String },
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
