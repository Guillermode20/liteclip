#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingLifecycle {
    Idle,
    Starting,
    Running,
    Stopping,
    Faulted,
}
