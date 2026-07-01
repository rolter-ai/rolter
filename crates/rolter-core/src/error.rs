use thiserror::Error;

/// Top-level error type shared across rolter crates.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("upstream error: {0}")]
    Upstream(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("store error: {0}")]
    Store(String),

    #[error("unauthorized")]
    Unauthorized,
}

/// Convenience alias for results that fail with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
