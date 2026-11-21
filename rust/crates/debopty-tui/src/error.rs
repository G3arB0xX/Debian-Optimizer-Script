use thiserror::Error;

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("terminal I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("bridge execution error: {0}")]
    Bridge(#[from] debopty_bridge::BridgeError),

    #[error("core error: {0}")]
    Core(#[from] debopty_core::CoreError),

    #[error("action failed: {0}")]
    ActionFailed(String),
}

pub type Result<T> = std::result::Result<T, TuiError>;
