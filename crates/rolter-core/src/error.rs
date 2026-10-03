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

    /// A write the store refused because another record already holds a value
    /// that must be unique (a name, slug or key). Carries the store's own
    /// description for the log; an API renders its own message instead.
    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("unauthorized")]
    Unauthorized,
}

/// Convenience alias for results that fail with [`enum@Error`].
pub type Result<T> = std::result::Result<T, Error>;
