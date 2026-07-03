/// Errors returned by broker startup and configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    /// Configuration was syntactically valid but unsafe or incomplete.
    #[error("config error: {0}")]
    Config(String),
    /// Filesystem or socket error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parse error.
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    /// Upstream HTTP client error.
    #[error("http client error: {0}")]
    Aioduct(#[from] aioduct::Error),
}

pub(crate) fn config_error(msg: impl Into<String>) -> BrokerError {
    BrokerError::Config(msg.into())
}
