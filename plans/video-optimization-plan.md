# AMF Encoder Optimization Plan

## Overview

Focused optimization plan for AMD AMF encoder to improve video quality and compression efficiency without sacrificing performance.

## Current AMF Implementation

**File:** [`src/encode/hw_encoder/types.rs`](src/encode/hw_encoder/types.rs:337-352)

```rust
"h264_amf" | "hevc_amf" | "av1_amf" => {
    cmd.arg("-quality").arg(self.amf_quality_mode());
    cmd.arg("-bf").arg("0");
    cmd.arg("-sei").arg("+aud");
    cmd.arg("-usage").arg("lowlatency");
    cmd.arg("-pa_adaptive_mini_gop").arg("0");
    cmd.arg("-header_insertion_mode").arg("idr");
    cmd.arg("-gops_per_idr").arg("1");
}
```

## Recommended AMF Optimizations

| Setting | Current | Optimized | Benefit |
|---------|---------|-----------|---------|
| Preanalysis | Not set | `1` | Better motion estimation for encoding decisions |
| VBAQ | Not set | `1` | Variance-based adaptive quantization for consistent quality |
| RC lookahead | Not set | `8` | Lookahead for better rate control decisions |
| Max QP difference | Not set | `4` | Smoother quality transitions between frames |

### Quality Improvement Breakdown

1. **Preanalysis** (`-preanalysis 1`): Enables motion estimation before encoding, allowing the encoder to make better decisions about where to allocate bits. Particularly beneficial for screen capture with rapid motion.

2. **VBAQ** (`-vbaq 1`): Variance-Based Adaptive Quantization analyzes frame complexity and adjusts quantization parameters locally. This results in:
   - Better quality in complex regions
   - Lower bitrate in simple regions
   - More consistent visual quality overall

3. **RC Lookahead** (`-rc_lookahead 8`): Gives the rate controller visibility into upcoming frames, enabling better bit allocation for:
   - Scene changes
   - Motion intensity variations
   - Keyframe positioning

4. **Max QP Difference** (`-max_qp_delta 4`): Limits quality variation between adjacent frames, reducing visible quality fluctuations during playback.

## Proposed Code Changes

```rust
"h264_amf" | "hevc_amf" | "av1_amf" => {
    cmd.arg("-quality").arg(self.amf_quality_mode());
    
    // CRITICAL: B-frames must stay disabled for h264_amf compatibility
    cmd.arg("-bf").arg("0");
    
    // Quality enhancement features
    cmd.arg("-preanalysis").arg("1");      // Motion estimation
    cmd.arg("-vbaq").arg("1");             // Variance-based AQ
    cmd.arg("-rc_lookahead").arg("8");     // RC lookahead depth
    cmd.arg("-max_qp_delta").arg("4");     // Smooth QP transitions
    
    // Header configuration for clean seeks
    cmd.arg("-sei").arg("+aud");
    cmd.arg("-header_insertion_mode").arg("idr");
    cmd.arg("-gops_per_idr").arg("1");
    
    // Low latency mode for replay buffer
    cmd.arg("-usage").arg("lowlatency");
    cmd.arg("-pa_adaptive_mini_gop").arg("0");
}
```

## Testing Requirements

1. **Quality Validation**
   - Record clips with identical settings before/after
   - Compare visual quality in motion-intensive scenes
   - Check for artifacts in gradients and text

2. **Performance Validation**
   - Verify no increase in encoding latency
   - Check GPU usage remains stable
   - Confirm no frame drops during recording

3. **Compatibility Testing**
   - Test h264_amf, hevc_amf, and av1_amf
   - Verify output plays correctly in media players
   - Test with different resolutions and framerates

## Expected Improvements

- **5-10% better compression** at same quality level
- **More consistent visual quality** across frame types
- **Better handling of motion** in screen capture content