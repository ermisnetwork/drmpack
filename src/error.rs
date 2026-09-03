use thiserror::Error;

#[derive(Error, Debug)]
pub enum DrmpackError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Key provider error: {0}")]
    KeyProvider(String),

    #[error("Encryption error: {0}")]
    Encryption(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("GPAC engine error: {0}")]
    Gpac(String),

    #[error("GPAC process crashed with exit code {exit_code:?}: {stderr}")]
    ProcessCrashed {
        exit_code: Option<i32>,
        stderr: String,
    },
}

pub type Result<T> = std::result::Result<T, DrmpackError>;
