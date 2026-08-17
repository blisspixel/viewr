//! Error types for viewr.

/// Errors that can occur while starting or running viewr.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The windowing event loop failed to start or run.
    #[error("event loop error: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),

    /// A GPU surface, adapter, or device could not be created.
    #[error("gpu initialization failed: {0}")]
    Gpu(String),

    /// The graphical session cannot present a window. The message is already a
    /// complete, actionable report, so it is printed without a prefix.
    #[error("{0}")]
    Launch(String),

    /// Native operating-system integration could not be initialized.
    #[error("platform integration failed: {0}")]
    Platform(String),

    /// An image file could not be read or decoded.
    #[error("could not open image: {0}")]
    Decode(String),

    /// An image could not be encoded or written to disk.
    #[error("could not save image: {0}")]
    Encode(String),
}

#[cfg(test)]
mod tests {
    use super::Error;

    /// Every variant is printed by the launch path, so every variant is checked.
    /// A launch report is read on a terminal, so it stays one line with no
    /// leading capital or trailing period of its own.
    #[test]
    fn each_error_reports_its_own_category_to_the_reader() {
        let launch = Error::Launch("no display session is reachable".into());
        for (error, expected) in [
            (
                Error::from(winit::error::EventLoopError::ExitFailure(3)),
                "event loop error: Exit Failure: 3",
            ),
            (
                Error::Gpu("create_surface failed".into()),
                "gpu initialization failed: create_surface failed",
            ),
            // A launch failure is already a complete report and takes no prefix.
            (launch, "no display session is reachable"),
            (
                Error::Platform("open-file handler unavailable".into()),
                "platform integration failed: open-file handler unavailable",
            ),
            (
                Error::Decode("unsupported format".into()),
                "could not open image: unsupported format",
            ),
            (
                Error::Encode("destination is read only".into()),
                "could not save image: destination is read only",
            ),
        ] {
            let reported = error.to_string();
            assert_eq!(reported, expected);
            assert!(!reported.contains('\n'), "{reported}");
            // Debug output names the variant for opt-in diagnostics.
            assert!(!format!("{error:?}").is_empty());
        }
    }
}
