use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid version: {0}")]
    InvalidVersion(String),

    #[error("invalid version bump: {0}")]
    InvalidBump(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("template not found: {0}")]
    TemplateNotFound(String),

    #[error("command '{prog}' failed: {message}")]
    CommandFailed { prog: String, message: String },

    #[error("command '{prog}' timed out after {secs}s")]
    CommandTimeout { prog: String, secs: u64 },

    #[error("validation failed: {0}")]
    ValidationFailed(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("HTTP {status} for {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("system error: {0}")]
    System(String),

    #[error("security error: {0}")]
    Security(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
