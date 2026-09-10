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

    #[error("GPAC process crashed with exit code {exit_code:?}: {stderr}{}", diagnose_gpac_crash(*exit_code, stderr))]
    ProcessCrashed {
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("PackagingSession failure: {0}")]
    PackagingSession(std::sync::Arc<PackagingSessionFailure>),

    #[cfg(feature = "license-proxy")]
    #[error(
        "License proxy error (HTTP {status}): {message}{}",
        format_license_diagnostic(diagnostic)
    )]
    LicenseProxy {
        status: reqwest::StatusCode,
        message: String,
        diagnostic: Option<String>,
    },
}

/// Analyze a GPAC process crash and return an actionable troubleshooting hint.
pub fn diagnose_gpac_crash(exit_code: Option<i32>, stderr: &str) -> &'static str {
    match exit_code {
        Some(127) => " [Hint: 'gpac' executable was not found in PATH. Ensure GPAC (>=2.2) is installed.]",
        Some(137) => " [Hint: GPAC was killed by SIGKILL (Exit code 137, OOM Killer). Check memory and /dev/shm headroom.]",
        Some(139) => " [Hint: GPAC crashed with Segmentation Fault (SIGSEGV). Check if input stream is a valid fMP4 container.]",
        Some(141) => " [Hint: Broken pipe (SIGPIPE). Upstream encoder or pipe writer closed prematurely.]",
        _ if stderr.contains("cecrypt") || stderr.contains("invalid key") => " [Hint: cecrypt DRM filter failure. Verify KeyID and ContentKey hex format and DRM XML.]",
        _ if stderr.contains("Cannot find filter") => " [Hint: GPAC is missing required filter modules. Check GPAC build flags.]",
        _ if stderr.contains("fail to fetch sample") => " [Hint: Container sample demux failure. Ensure input fMP4 is clean and all declared tracks are present.]",
        _ => "",
    }
}

fn format_license_diagnostic(diagnostic: &Option<String>) -> String {
    match diagnostic {
        Some(diag) if !diag.is_empty() => format!(" [Diagnostic: {diag}]"),
        _ => String::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnose_gpac_crash_hints() {
        assert!(diagnose_gpac_crash(Some(127), "").contains("PATH"));
        assert!(diagnose_gpac_crash(Some(137), "").contains("OOM"));
        assert!(diagnose_gpac_crash(Some(139), "").contains("Segmentation Fault"));
        assert!(diagnose_gpac_crash(Some(141), "").contains("Broken pipe"));
        assert!(diagnose_gpac_crash(None, "error in cecrypt filter").contains("cecrypt"));
        assert!(
            diagnose_gpac_crash(None, "Track #2 fail to fetch sample").contains("demux failure")
        );
    }

    #[test]
    fn test_process_crashed_display_includes_hint() {
        let err = DrmpackError::ProcessCrashed {
            exit_code: Some(127),
            stderr: "not found".into(),
        };
        let formatted = format!("{err}");
        assert!(formatted.contains("not found"));
        assert!(formatted.contains("Hint: 'gpac' executable was not found in PATH"));
    }

    #[cfg(feature = "license-proxy")]
    #[test]
    fn test_license_proxy_error_display_with_diagnostic() {
        let err = DrmpackError::LicenseProxy {
            status: reqwest::StatusCode::FORBIDDEN,
            message: "Denied".into(),
            diagnostic: Some("Token expired".into()),
        };
        let formatted = format!("{err}");
        assert!(formatted.contains("HTTP 403"));
        assert!(formatted.contains("Denied"));
        assert!(formatted.contains("Diagnostic: Token expired"));
    }
}
