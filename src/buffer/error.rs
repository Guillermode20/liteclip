use thiserror::Error;

#[derive(Debug, Error)]
pub enum BufferError {
    #[error("Buffer capacity exceeded")]
    CapacityExceeded,

    #[error("No keyframe available")]
    NoKeyframe,

    #[error("Buffer is empty")]
    Empty,
}
