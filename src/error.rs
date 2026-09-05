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

    #[error("PackagingSession failure: {0}")]
    PackagingSession(std::sync::Arc<PackagingSessionFailure>),

    #[cfg(feature = "license-proxy")]
    #[error("License proxy error (HTTP {status}): {message}")]
    LicenseProxy {
        status: reqwest::StatusCode,
        message: String,
        diagnostic: Option<String>,
    },
}

/// The lifecycle operation performed on an encryption Representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackagingOperation {
    Create,
    Write,
    Status,
    Watchdog,
    Close,
    Supervisor,
}

impl std::fmt::Display for PackagingOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackagingOperation::Create => write!(f, "create"),
            PackagingOperation::Write => write!(f, "write"),
            PackagingOperation::Status => write!(f, "status check"),
            PackagingOperation::Watchdog => write!(f, "watchdog shutdown"),
            PackagingOperation::Close => write!(f, "close"),
            PackagingOperation::Supervisor => write!(f, "supervisor exit"),
        }
    }
}

/// A failure originating from one concrete encryption Representation.
#[derive(Debug)]
pub struct RepresentationFailure {
    pub scheme: crate::types::EncryptionScheme,
    pub operation: PackagingOperation,
    pub error: DrmpackError,
}

impl RepresentationFailure {
    pub(crate) fn new(
        scheme: crate::types::EncryptionScheme,
        operation: PackagingOperation,
        error: DrmpackError,
    ) -> Self {
        debug_assert!(
            matches!(
                scheme,
                crate::types::EncryptionScheme::Cenc | crate::types::EncryptionScheme::Cbcs
            ),
            "Dual is an orchestration mode, not a Representation"
        );
        Self {
            scheme,
            operation,
            error,
        }
    }
}

impl std::fmt::Display for RepresentationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} Representation failed during {}: {}",
            self.scheme, self.operation, self.error
        )
    }
}

/// The terminal failure of a PackagingSession, including every Representation failure observed.
#[derive(Debug)]
pub struct PackagingSessionFailure {
    pub cenc: Vec<RepresentationFailure>,
    pub cbcs: Vec<RepresentationFailure>,
    pub output_cleanup: Option<DrmpackError>,
    pub control_cleanup: Option<DrmpackError>,
}

impl PackagingSessionFailure {
    pub(crate) fn from_failures(failures: Vec<RepresentationFailure>) -> Self {
        let mut cenc = Vec::new();
        let mut cbcs = Vec::new();
        for failure in failures {
            match failure.scheme {
                crate::types::EncryptionScheme::Cenc => cenc.push(failure),
                crate::types::EncryptionScheme::Cbcs => cbcs.push(failure),
                crate::types::EncryptionScheme::Dual => {
                    unreachable!("Dual cannot be a concrete representation")
                }
            }
        }
        Self {
            cenc,
            cbcs,
            output_cleanup: None,
            control_cleanup: None,
        }
    }

    pub(crate) fn with_cleanup_failures(
        mut self,
        output_cleanup: Option<DrmpackError>,
        control_cleanup: Option<DrmpackError>,
    ) -> Self {
        self.output_cleanup = output_cleanup;
        self.control_cleanup = control_cleanup;
        self
    }

    pub fn failures(&self) -> impl Iterator<Item = &RepresentationFailure> {
        self.cenc.iter().chain(self.cbcs.iter())
    }
}

impl std::fmt::Display for PackagingSessionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut messages = self.failures().map(ToString::to_string).collect::<Vec<_>>();
        if let Some(error) = &self.output_cleanup {
            messages.push(format!("output cleanup failed: {error}"));
        }
        if let Some(error) = &self.control_cleanup {
            messages.push(format!("control cleanup failed: {error}"));
        }
        write!(f, "{}", messages.join("; "))
    }
}

impl std::error::Error for PackagingSessionFailure {}

pub type Result<T> = std::result::Result<T, DrmpackError>;
