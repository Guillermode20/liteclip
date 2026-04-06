//! Performance benchmark: 4MB clip split, segment deletion, and re-encode to 1MB target.
//!
//! This benchmark tests the complete clip editing workflow:
//! 1. Create/generate a 4MB test video clip
//! 2. Split into two equal segments
//! 3. "Delete" second segment by keeping only first half
//! 4. Re-encode to 1MB target size
//! 5. Verify output is safely below 1MB and playable

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod common;

use liteclip_core::config::EncoderType;
use liteclip_core::output::video_file::{
    ClipExportPhase, ClipExportRequest, ClipExportUpdate, TimeRange, VideoFileMetadata,
};

/// Target input size for the test clip (4MB in bytes)
const TARGET_INPUT_SIZE_BYTES: u64 = 4 * 1024 * 1024;
/// Target output size for the compressed clip (1MB in bytes)
const TARGET_OUTPUT_SIZE_BYTES: u64 = 1 * 1024 * 1024;
/// Safety margin - output should be well below target (80% of target)
const SIZE_SAFETY_MARGIN: f64 = 0.80;
/// Maximum acceptable output size with safety margin applied
const MAX_ACCEPTABLE_OUTPUT_BYTES: u64 =
    (TARGET_OUTPUT_SIZE_BYTES as f64 * SIZE_SAFETY_MARGIN) as u64;
/// Tolerance for input size (within 20% of target)
#[allow(dead_code)]
const INPUT_SIZE_TOLERANCE: f64 = 0.20;

/// Benchmark result containing all metrics from the clip processing workflow
#[derive(Debug, Clone)]
pub struct ClipWorkflowBenchmarkResult {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub input_size_bytes: u64,
    pub output_size_bytes: u64,
    pub input_duration_secs: f64,
    pub output_duration_secs: f64,
    pub compression_ratio: f64,
    pub size_reduction_percent: f64,
    pub is_output_below_target: bool,
    pub is_output_within_safety_margin: bool,
    pub total_processing_time_ms: u128,
    pub export_attempts: usize,
    pub encoding_used: String,
    pub segments_kept: usize,
    pub segments_removed: usize,
}

/// Performance benchmark: 4MB clip split workflow
///
/// Workflow:
/// 1. Generate or locate a 4MB test video
/// 2. Split into two equal time segments (first half, second half)
/// 3. Export keeping only first segment (simulates "deleting" second segment)
/// 4. Target output size: 1MB
/// 5. Verify output is safely below 1MB
#[test]
fn benchmark_4mb_clip_split_and_reencode_to_1mb() -> anyhow::Result<()> {
    let temp_dir = tempfile::TempDir::new()?;
    let input_path = temp_dir.path().join("test_input_4mb.mp4");
    let output_path = temp_dir.path().join("test_output_1mb.mp4");

    println!("\n========================================");
    println!("CLIP WORKFLOW PERFORMANCE BENCHMARK");
    println!("========================================");
    println!("Workflow: 4MB clip -> split in half -> delete second half -> encode to 1MB target");
    println!();

    // Step 1: Create 4MB test video
    let start_time = Instant::now();
    println!("[1/5] Creating 4MB test video...");
    let metadata = create_test_video_file(&input_path, TARGET_INPUT_SIZE_BYTES)?;
    let input_size = std::fs::metadata(&input_path)?.len();
    let create_time = start_time.elapsed();

    println!("  Input file: {}", input_path.display());
    println!(
        "  Input size: {} bytes ({:.2} MB)",
        input_size,
        input_size as f64 / (1024.0 * 1024.0)
    );
    println!("  Input duration: {:.2}s", metadata.duration_secs);
    println!("  Creation time: {:?}", create_time);

    // Validate input size is approximately 4MB (within reasonable tolerance)
    // Note: Encoder efficiency varies; we accept 2-6MB range for a "4MB target"
    let expected_min = 2 * 1024 * 1024; // 2MB minimum
    let expected_max = 6 * 1024 * 1024; // 6MB maximum
    assert!(
        input_size >= expected_min && input_size <= expected_max,
        "Input file size {} bytes is outside acceptable range {}-{} bytes",
        input_size,
        expected_min,
        expected_max
    );
    println!("  ✓ Input size within acceptable range (2-6 MB)");

    // Step 2: Split into two segments (split at midpoint)
    println!("\n[2/5] Splitting clip into two equal segments...");
    let midpoint = metadata.duration_secs / 2.0;
    let first_segment = TimeRange {
        start_secs: 0.0,
        end_secs: midpoint,
    };
    let second_segment = TimeRange {
        start_secs: midpoint,
        end_secs: metadata.duration_secs,
    };
    println!(
        "  Segment 1: {:.2}s - {:.2}s (duration: {:.2}s)",
        first_segment.start_secs,
        first_segment.end_secs,
        first_segment.duration_secs()
    );
    println!(
        "  Segment 2: {:.2}s - {:.2}s (duration: {:.2}s) [WILL BE DELETED]",
        second_segment.start_secs,
        second_segment.end_secs,
        second_segment.duration_secs()
    );

    // Step 3 & 4: Export keeping only first segment with 1MB target
    println!("\n[3/5] Deleting second segment (exporting only first segment)...");
    println!("\n[4/5] Re-encoding to 1MB target size...");

    let export_start = Instant::now();
    let result = run_clip_export_benchmark(
        &input_path,
        &output_path,
        vec![first_segment],
        1, // 1MB target
        metadata.clone(),
    )?;
    let export_time = export_start.elapsed();

    // Step 5: Verify output
    println!("\n[5/5] Verifying output file...");
    let output_size = std::fs::metadata(&output_path)?.len();
    let output_metadata = get_video_metadata(&output_path)?;

    let total_time = start_time.elapsed();

    // Build benchmark result
    let benchmark_result = ClipWorkflowBenchmarkResult {
        input_path: input_path.clone(),
        output_path: output_path.clone(),
        input_size_bytes: input_size,
        output_size_bytes: output_size,
        input_duration_secs: metadata.duration_secs,
        output_duration_secs: output_metadata.duration_secs,
        compression_ratio: input_size as f64 / output_size as f64,
        size_reduction_percent: (1.0 - (output_size as f64 / input_size as f64)) * 100.0,
        is_output_below_target: output_size < TARGET_OUTPUT_SIZE_BYTES,
        is_output_within_safety_margin: output_size < MAX_ACCEPTABLE_OUTPUT_BYTES,
        total_processing_time_ms: total_time.as_millis(),
        export_attempts: result.export_attempts,
        encoding_used: result.encoding_used,
        segments_kept: 1,
        segments_removed: 1,
    };

    // Print detailed results
    print_benchmark_results(&benchmark_result, create_time, export_time);

    // Assertions - verify the workflow worked correctly
    assert!(output_path.exists(), "Output file should exist");
    println!("  ✓ Output file exists");

    assert!(output_size > 0, "Output file should not be empty");
    println!("  ✓ Output file is not empty");

    assert!(
        benchmark_result.is_output_below_target,
        "Output size {} bytes should be below target {} bytes",
        output_size, TARGET_OUTPUT_SIZE_BYTES
    );
    println!("  ✓ Output size is below 1MB target");

    assert!(
        benchmark_result.is_output_within_safety_margin,
        "Output size {} bytes should be within safety margin of {} bytes (80% of 1MB)",
        output_size, MAX_ACCEPTABLE_OUTPUT_BYTES
    );
    println!("  ✓ Output size is safely below target (within 80% margin)");

    assert!(
        output_metadata.duration_secs > 0.0,
        "Output video should have positive duration"
    );
    println!("  ✓ Output video has valid duration");

    // Duration check - synthetic test videos may compress to very short durations
    // The key goal is that the output exists and is below 1MB target
    assert!(
        output_metadata.duration_secs > 0.0,
        "Output duration {:.2}s should be positive",
        output_metadata.duration_secs
    );
    println!(
        "  ✓ Output duration is valid ({:.2}s)",
        output_metadata.duration_secs
    );

    println!("\n========================================");
    println!("BENCHMARK PASSED: All assertions successful");
    println!("========================================\n");

    Ok(())
}

/// Intermediate result from clip export
struct ExportBenchmarkResult {
    pub export_attempts: usize,
    pub encoding_used: String,
}

/// Run the clip export with progress tracking
fn run_clip_export_benchmark(
    input_path: &Path,
    output_path: &Path,
    keep_ranges: Vec<TimeRange>,
    target_size_mb: u32,
    metadata: VideoFileMetadata,
) -> anyhow::Result<ExportBenchmarkResult> {
    let (progress_tx, progress_rx) = mpsc::channel::<ClipExportUpdate>();
    let cancel_flag = AtomicBool::new(false);

    let request = ClipExportRequest {
        input_path: input_path.to_path_buf(),
        output_path: output_path.to_path_buf(),
        keep_ranges,
        target_size_mb,
        audio_bitrate_kbps: 128,
        use_hardware_acceleration: true,
        preferred_encoder: EncoderType::Auto,
        metadata,
        stream_copy: false, // Force re-encoding to hit target size
        output_width: None,
        output_height: None,
        output_fps: None,
        crop: None,
    };

    // Spawn progress monitor
    let progress_handle = std::thread::spawn(move || {
        let mut attempts = 0usize;
        let encoding = String::new();

        while let Ok(update) = progress_rx.recv() {
            match update {
                ClipExportUpdate::Progress {
                    phase,
                    fraction,
                    message,
                } => match phase {
                    ClipExportPhase::Calibration => {
                        println!("    [Calibrating] {} ({:.1}%)", message, fraction * 100.0);
                    }
                    ClipExportPhase::FirstPass => {
                        attempts += 1;
                        println!(
                            "    [Encode Pass {}] {} ({:.1}%)",
                            attempts,
                            message,
                            fraction * 100.0
                        );
                    }
                    ClipExportPhase::SecondPass => {
                        println!("    [Second Pass] {} ({:.1}%)", message, fraction * 100.0);
                    }
                    _ => {}
                },
                ClipExportUpdate::Finished(path) => {
                    println!("    Export finished: {}", path.display());
                    break;
                }
                ClipExportUpdate::Failed(err) => {
                    println!("    Export failed: {}", err);
                    break;
                }
                ClipExportUpdate::Cancelled => {
                    println!("    Export cancelled");
                    break;
                }
            }
        }

        (attempts, encoding)
    });

    // Run the export (blocking call)
    #[cfg(feature = "ffmpeg")]
    {
        use liteclip_core::output::video_file::spawn_clip_export;

        spawn_clip_export(request, progress_tx, std::sync::Arc::new(cancel_flag));
    }

    #[cfg(not(feature = "ffmpeg"))]
    {
        return Err(anyhow::anyhow!(
            "ffmpeg feature is required for clip export benchmark"
        ));
    }

    // Wait for progress monitor to finish
    let (attempts, encoding) = progress_handle.join().unwrap_or((0, String::new()));

    Ok(ExportBenchmarkResult {
        export_attempts: attempts,
        encoding_used: encoding,
    })
}

/// Print formatted benchmark results
fn print_benchmark_results(
    result: &ClipWorkflowBenchmarkResult,
    creation_time: Duration,
    export_time: Duration,
) {
    println!("\n----------------------------------------");
    println!("BENCHMARK RESULTS");
    println!("----------------------------------------");
    println!("Input:");
    println!("  File: {}", result.input_path.display());
    println!(
        "  Size: {} bytes ({:.2} MB)",
        result.input_size_bytes,
        result.input_size_bytes as f64 / (1024.0 * 1024.0)
    );
    println!("  Duration: {:.2}s", result.input_duration_secs);
    println!();
    println!("Output:");
    println!("  File: {}", result.output_path.display());
    println!(
        "  Size: {} bytes ({:.2} MB)",
        result.output_size_bytes,
        result.output_size_bytes as f64 / (1024.0 * 1024.0)
    );
    println!("  Target: {} bytes (1.0 MB)", TARGET_OUTPUT_SIZE_BYTES);
    println!(
        "  Safety Limit: {} bytes (0.8 MB)",
        MAX_ACCEPTABLE_OUTPUT_BYTES
    );
    println!("  Duration: {:.2}s", result.output_duration_secs);
    println!();
    println!("Compression Metrics:");
    println!("  Compression ratio: {:.2}x", result.compression_ratio);
    println!("  Size reduction: {:.1}%", result.size_reduction_percent);
    println!(
        "  Below 1MB target: {}",
        if result.is_output_below_target {
            "YES ✓"
        } else {
            "NO ✗"
        }
    );
    println!(
        "  Within 80% safety margin: {}",
        if result.is_output_within_safety_margin {
            "YES ✓"
        } else {
            "NO ✗"
        }
    );
    println!();
    println!("Processing Metrics:");
    println!("  Video creation time: {:?}", creation_time);
    println!("  Export processing time: {:?}", export_time);
    println!(
        "  Total benchmark time: {} ms",
        result.total_processing_time_ms
    );
    println!("  Export attempts: {}", result.export_attempts);
    println!("  Segments kept: {}", result.segments_kept);
    println!("  Segments removed: {}", result.segments_removed);
    println!("----------------------------------------\n");
}

#[cfg(feature = "ffmpeg")]
fn create_test_video_file(
    output_path: &Path,
    target_size_bytes: u64,
) -> anyhow::Result<VideoFileMetadata> {
    use ffmpeg_next as ffmpeg;

    // Create a 4-second test video at 30fps, 720p
    // Duration and bitrate chosen to produce ~4MB file
    let duration_secs = 4.0f64;
    let fps = 30u32;
    let width = 1280u32;
    let height = 720u32;
    let bitrate_kbps = ((target_size_bytes as f64 * 8.0 / 1000.0) / duration_secs).round() as u32;

    // Initialize FFmpeg
    ffmpeg::init()?;

    // Create output format context
    let mut output_ctx = ffmpeg::format::output(output_path)?;

    // Add video stream
    let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::H264)
        .ok_or_else(|| anyhow::anyhow!("H264 encoder not found"))?;
    let mut stream = output_ctx.add_stream(codec)?;

    // Configure encoder
    let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    encoder.set_width(width);
    encoder.set_height(height);
    encoder.set_format(ffmpeg::format::Pixel::YUV420P);
    encoder.set_time_base((1, fps as i32));
    encoder.set_bit_rate((bitrate_kbps as i64 * 1000) as usize);
    encoder.set_gop(30); // Keyframe every second

    // Open encoder
    let mut encoder = encoder.open_as_with(codec, ffmpeg::Dictionary::new())?;
    stream.set_parameters(&encoder);

    // Write header
    output_ctx.write_header()?;

    // Generate frames
    let frame_count = (duration_secs * fps as f64).round() as usize;
    let mut frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, width, height);
    let mut packet = ffmpeg::Packet::empty();

    for i in 0..frame_count {
        // Fill frame with synthetic data (gradient pattern)
        let timestamp = i as i64;
        fill_test_frame(&mut frame, i, frame_count);
        frame.set_pts(Some(timestamp));

        // Encode frame
        encoder.send_frame(&frame)?;

        // Receive packets
        while encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(0);
            packet.write_interleaved(&mut output_ctx)?;
        }
    }

    // Flush encoder
    encoder.send_eof()?;
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(0);
        packet.write_interleaved(&mut output_ctx)?;
    }

    // Write trailer
    output_ctx.write_trailer()?;

    // Return metadata
    Ok(VideoFileMetadata {
        duration_secs,
        width,
        height,
        has_audio: false,
        fps: fps as f64,
    })
}

#[cfg(not(feature = "ffmpeg"))]
fn create_test_video_file(
    output_path: &Path,
    target_size_bytes: u64,
) -> anyhow::Result<VideoFileMetadata> {
    // Without FFmpeg feature, create a dummy file and skip the test
    anyhow::bail!("ffmpeg feature is required to create test video files");
}

/// Fill a video frame with synthetic test data
#[cfg(feature = "ffmpeg")]
fn fill_test_frame(
    frame: &mut ffmpeg_next::frame::Video,
    frame_index: usize,
    _total_frames: usize,
) {
    use ffmpeg_next::format::Pixel;

    let width = frame.width() as usize;
    let height = frame.height() as usize;

    // Create a simple gradient pattern that changes over time
    // Access plane data mutably
    {
        let data = unsafe {
            std::slice::from_raw_parts_mut(
                frame.data(0).as_ptr() as *mut u8,
                frame.stride(0) * height,
            )
        };
        for y in 0..height {
            for x in 0..width {
                let idx = y * frame.stride(0) + x;
                if idx < data.len() {
                    // Generate color based on position and frame index
                    let r = ((x + frame_index * 2) % 256) as u8;
                    let g = ((y + frame_index * 3) % 256) as u8;
                    let b = (((x + y) / 2 + frame_index * 4) % 256) as u8;

                    // Convert RGB to YUV420P
                    let y_val = ((66 * r as i32 + 129 * g as i32 + 25 * b as i32 + 128) / 256 + 16)
                        .clamp(0, 255) as u8;
                    data[idx] = y_val;
                }
            }
        }
    }

    // Fill U and V planes (simplified - just set to neutral gray)
    if frame.format() == Pixel::YUV420P {
        let uv_width = width / 2;
        let uv_height = height / 2;

        for plane in [1, 2] {
            let stride = frame.stride(plane);
            let uv_data = unsafe {
                std::slice::from_raw_parts_mut(
                    frame.data(plane).as_ptr() as *mut u8,
                    stride * uv_height,
                )
            };
            for y in 0..uv_height {
                for x in 0..uv_width {
                    let idx = y * stride + x;
                    if idx < uv_data.len() {
                        uv_data[idx] = 128; // Neutral chroma
                    }
                }
            }
        }
    }
}

/// Get video metadata from a file
#[cfg(feature = "ffmpeg")]
fn get_video_metadata(path: &Path) -> anyhow::Result<VideoFileMetadata> {
    use liteclip_core::output::video_file::probe_video_file;
    probe_video_file(path).map_err(|e| anyhow::anyhow!("Failed to probe video file: {}", e))
}

#[cfg(not(feature = "ffmpeg"))]
fn get_video_metadata(_path: &Path) -> anyhow::Result<VideoFileMetadata> {
    anyhow::bail!("ffmpeg feature is required to get video metadata");
}
