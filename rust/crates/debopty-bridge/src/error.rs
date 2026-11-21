use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("core error: {0}")]
    Core(#[from] debopty_core::CoreError),

    #[error("unable to locate debopty script directory: {0}")]
    ScriptDirNotFound(String),

    #[error("required Bash module not found: {path}")]
    MissingScript { path: PathBuf },

    #[error("unknown bridge action: {0}")]
    UnknownAction(String),

    #[error("unknown engine mode: {0}")]
    UnknownEngineMode(String),
}

pub type Result<T> = std::result::Result<T, BridgeError>;
