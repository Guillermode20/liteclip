# LiteClip Recorder - CPU Performance Optimization Plan

## Overview

This document outlines performance optimization opportunities identified in the LiteClip Recorder codebase. The focus is on reducing CPU usage while maintaining or improving functionality, particularly in the most computationally expensive sections.

## Analysis Summary

The LiteClip Recorder is a screen recording application with the following key components:
- DXGI-based screen capture
- Multiple encoding options (software, AMD AMF, NVIDIA NVENC, Intel QSV)
- Audio capture (system and microphone)
- Replay buffer management
- Video export and preview functionality

## Performance-Critical Sections

### 1. Software Encoder BGRA to RGB Conversion and Scaling
**File:** [`src/encode/sw_encoder.rs`](../src/encode/sw_encoder.rs)

**Current Implementation:**
- Uses manual bilinear scaling with fixed-point arithmetic
- Performs BGRA to RGB channel swapping
- Implements software JPEG encoding using the `image` crate
- Runs in a multi-threaded worker pool

**Optimization Opportunities:**
- Replace manual scaling with SIMD-accelerated scaling (using `simd` or `imageproc` crates)
- Optimize channel swapping using SIMD instructions
- Consider using hardware-accelerated JPEG encoding if available
- Improve cache efficiency by reordering pixel operations

### 2. Noise Suppression Processing
**File:** [`src/capture/audio/mic.rs`](../src/capture/audio/mic.rs)

**Current Implementation:**
- Uses `nnnoiseless` (RNNoise) library for noise suppression
- Implements overlap-add processing with Hann window
- Performs DC blocking, gain smoothing, and soft limiting
- Processes audio in 480-sample frames (10ms at 48kHz)

**Optimization Opportunities:**
- Optimize noise suppressor initialization and per-frame processing
- Improve cache efficiency by reusing buffers
- Consider using SIMD acceleration for audio processing
- Optimize the PRNG used for comfort noise generation

### 3. Lock-Free Buffer Parameter Set Caching
**File:** [`src/buffer/ring/lockfree.rs`](../src/buffer/ring/lockfree.rs)

**Current Implementation:**
- Caches H.264 SPS/PPS and HEVC VPS/SPS/PPS
- Scans each video packet for parameter set NAL units
- Uses a mutex to protect the parameter cache
- Implements a complete flag to stop scanning once all parameters are cached

**Optimization Opportunities:**
- Improve NAL unit detection algorithm
- Optimize parameter cache access with atomic operations instead of a mutex
- Reduce redundant scanning once parameter set cache is complete
- Improve memory management for cached parameter sets

### 4. Gallery Decode Pipeline Scaling and Conversion
**File:** [`src/gui/gallery/decode_pipeline.rs`](../src/gui/gallery/decode_pipeline.rs)

**Current Implementation:**
- Uses FFmpeg for video decoding
- Implements adaptive resolution scaling (low/medium/high quality)
- Handles frame queue management for playback
- Converts decoded frames to RGBA for display

**Optimization Opportunities:**
- Optimize FFmpeg decoder initialization and frame decoding
- Improve frame scaling using SIMD-accelerated algorithms
- Optimize frame queue operations to reduce contention
- Consider using hardware-accelerated decoding if available

### 5. FFmpeg Software Scaling in Encoder Initialization
**File:** [`src/encode/ffmpeg/software.rs`](../src/encode/ffmpeg/software.rs)

**Current Implementation:**
- Initializes FFmpeg encoder with scaling context
- Uses point scaling by default for speed
- Handles pixel format conversion (BGRA to NV12, etc.)
- Manages encoder frame buffers

**Optimization Opportunities:**
- Optimize scaling context initialization
- Consider using faster scaling algorithms for software encoding
- Improve buffer management for encoder frames
- Optimize pixel format conversion operations

### 6. Audio Buffer Management and Processing
**Files:** [`src/capture/audio/mic.rs`](../src/capture/audio/mic.rs), [`src/capture/audio/system.rs`](../src/capture/audio/system.rs)

**Current Implementation:**
- Uses `BytesMut` for audio buffer management
- Handles WASAPI audio capture and processing
- Implements packetization of captured audio
- Supports both system audio (loopback) and microphone capture

**Optimization Opportunities:**
- Improve audio buffer allocation and reuse
- Optimize packetization logic to reduce overhead
- Consider using SIMD acceleration for audio processing
- Reduce contention in audio packet channels

### 7. Frame Queue Operations
**Files:** [`src/capture/dxgi/capture.rs`](../src/capture/dxgi/capture.rs), [`src/encode/mod.rs`](../src/encode/mod.rs)

**Current Implementation:**
- Uses crossbeam channels for frame transport
- Implements backpressure mechanism for encoder overload
- Handles frame dropping and duplication for timeout cases
- Manages queue depth and adaptive throttling

**Optimization Opportunities:**
- Optimize channel operations to reduce overhead
- Improve backpressure algorithm for better responsiveness
- Reduce contention in frame queue operations
- Optimize adaptive throttling logic

## Implementation Strategy

1. **Profile First:** Use profiling tools to measure CPU usage and confirm the impact of optimizations
2. **Start Small:** Implement optimizations in isolated sections to minimize regression risk
3. **Test Thoroughly:** Run existing tests and add new tests for optimized sections
4. **Benchmark:** Measure performance improvements using real-world scenarios
5. **Iterate:** Continuously monitor and optimize based on performance feedback

## Tools and Technologies

- **Profiling:** Use `perf` on Linux or Windows Performance Analyzer
- **SIMD:** Use Rust's `std::simd` or third-party crates like `wide`
- **Image Processing:** Use optimized libraries like `imageproc` or `vips`
- **Audio Processing:** Use SIMD-accelerated audio libraries
- **Benchmarking:** Use `criterion` for microbenchmarking

## Timeline

The implementation will follow an iterative approach, with each optimization being developed and tested independently. The exact timeline will depend on the complexity of each optimization and available resources.

## Expected Benefits

By implementing these optimizations, we expect to see:
- Reduced CPU usage during recording
- Improved encoding performance
- Better battery life on mobile devices
- Smoother playback in the gallery
- More efficient use of system resources
