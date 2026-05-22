use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("RSS feed parsing failed: {0}")]
    Rss(#[from] rss::Error),

    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("file I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("Telegram API returned HTTP {status}: {body}")]
    TelegramApi { status: u16, body: String },

    #[error("invalid configuration value for '{key}': {value}")]
    InvalidConfig { key: &'static str, value: String },

    #[error("configuration key '{0}' is required but not set")]
    MissingConfig(&'static str),

    #[error("unknown provider: '{0}'")]
    UnknownProvider(String),

    #[error("unknown notifier: '{0}'")]
    UnknownNotifier(String),

    #[error("no providers enabled")]
    NoProvidersEnabled,

    #[error("no notifiers enabled")]
    NoNotifiersEnabled,

    #[error("HTTP client initialization failed: {0}")]
    ClientInit(String),
}
