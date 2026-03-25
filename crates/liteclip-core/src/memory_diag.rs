//! Process working-set / private bytes and replay-buffer stats for diagnosing memory retention
//! and slow drift while recording.
//!
//! When [`crate::buffer::ReplayBuffer`] counters stay flat but process memory keeps rising,
//! use native tools on a long session: Windows Performance Recorder (heap), Visual Studio
//! diagnostics heap snapshots, or Intel VTune memory growth, to attribute usage to
//! AMF/D3D11/WASAPI/libav outside the ring buffer.

use crate::buffer::ring::SharedReplayBuffer;
use tracing::info;

/// Working set and private usage in megabytes (Windows). `None` on non-Windows.
#[cfg(target_os = "windows")]
pub fn process_memory_mb() -> Option<(f64, f64)> {
    use std::mem::size_of;
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        if K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .as_bool()
        {
            return Some((
                counters.WorkingSetSize as f64 / (1024.0 * 1024.0),
                counters.PrivateUsage as f64 / (1024.0 * 1024.0),
            ));
        }
    }

    None
}

#[cfg(not(target_os = "windows"))]
pub fn process_memory_mb() -> Option<(f64, f64)> {
    None
}

/// Logs ring usage ([`SharedReplayBuffer::stats`]), pinned snapshot bytes, and process memory.
/// Intended for periodic calls from the encoder thread during recording.
pub fn log_recording_memory(stage: &str, buffer: &SharedReplayBuffer) {
    let stats = buffer.stats();
    let pinned = buffer.pinned_bytes();
    if let Some((working_set_mb, private_mb)) = process_memory_mb() {
        info!(
            "Recording memory [{}]: process_working={:.1}MB, private={:.1}MB, buffer={:.1}MB ({}pkts, {}kf, mem={:.0}%), pinned_snapshots={:.1}MB",
            stage,
            working_set_mb,
            private_mb,
            stats.total_bytes as f64 / 1_048_576.0,
            stats.packet_count,
            stats.keyframe_count,
            stats.memory_usage_percent,
            pinned as f64 / 1_048_576.0,
        );
    } else {
        info!(
            "Recording memory [{}]: buffer={:.1}MB ({}pkts, {}kf, mem={:.0}%), pinned_snapshots={:.1}MB",
            stage,
            stats.total_bytes as f64 / 1_048_576.0,
            stats.packet_count,
            stats.keyframe_count,
            stats.memory_usage_percent,
            pinned as f64 / 1_048_576.0,
        );
    }
}
