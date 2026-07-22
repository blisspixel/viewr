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
