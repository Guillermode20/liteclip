use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("Muxer error: {0}")]
    MuxerError(String),

    #[error("File write error: {0}")]
    FileWriteError(#[from] std::io::Error),

    #[error("No video packets")]
    NoVideoPackets,
}
