# Audio Mixing Improvements Plan for LiteClip Recorder

## Current Architecture

The current audio pipeline in LiteClip Recorder consists of:

```
┌─────────────────┐    ┌─────────────────┐
│ System Capture  │    │ Mic Capture     │
│ (WASAPI Loopback)│    │ (WASAPI Capture)│
└────────┬────────┘    └────────┬────────┘
         │                     │
         │ [EncodedPacket]     │ [EncodedPacket]
         │                     │
         └──────────┬──────────┘
                    │
            ┌───────▼───────┐
            │ Audio Manager │
            │  Volume Scaling
            │  (per stream) │
            └───────┬───────┘
                    │
                    ▼
            ┌──────────────┐
            │ Replay Buffer│
            └──────────────┘
```

During MP4 muxing, audio packets are mixed:

```
┌──────────────────────────────────────┐
│ Mix Audio Packets to PCM             │
│ - Separate streams by type           │
│ - Sum samples with soft clipping     │
└──────────────────────────────────────┘
```

## Key Issues

1. **Separate Capture Paths**: System and microphone audio are captured separately and only mixed during muxing
2. **No Real-time Mixing**: No way to monitor mixed audio or adjust levels during capture
3. **Limited Volume Control**: Only per-stream volume, no master volume or balance
4. **Basic Clipping**: Simple soft clipping algorithm
5. **No Effects Processing**: No compression, limiting, or EQ
6. **Potential Desynchronization**: Separate capture timestamps could drift

## Proposed Architecture

```
┌─────────────────┐    ┌─────────────────┐
│ System Capture  │    │ Mic Capture     │
│ (WASAPI Loopback)│    │ (WASAPI Capture)│
└────────┬────────┘    └────────┬────────┘
         │                     │
         │ [Raw PCM]           │ [Raw PCM]
         │                     │
         └──────────┬──────────┘
                    │
            ┌───────▼────────┐
            │ Audio Mixer    │
            │ - Volume scaling
            │ - Balance control
            │ - Compression
            │ - Limiting
            │ - Soft clipping
            └───────┬────────┘
                    │
                    ▼
            ┌──────────────┐
            │ Replay Buffer│
            └──────────────┘
```

## Implementation Plan

### 1. Design Unified Audio Mixing System

**Goals**:
- Create a single audio processing pipeline
- Handle both system and microphone audio in a synchronized manner
- Support real-time mixing and monitoring

**Changes**:
- Modify `WasapiAudioManager` to mix audio during capture instead of forwarding separate packets
- Create a new `AudioMixer` struct that handles all audio processing
- Define a common audio format for all streams (48kHz, 16-bit stereo)

### 2. Implement Real-time Audio Mixing During Capture

**Goals**:
- Mix system and microphone audio in real-time
- Apply volume and balance controls
- Ensure synchronization between streams

**Changes**:
- Modify `forward_loop` in `WasapiAudioManager` to mix packets instead of forwarding separately
- Implement timestamp alignment for system and microphone packets
- Create a mixing buffer to handle overlapping audio segments

### 3. Add Master Volume and Balance Controls

**Goals**:
- Add master volume control for mixed audio
- Add balance control between system and microphone
- Extend configuration system to support new audio settings

**Changes**:
- Update `AudioConfig` struct in `src/config/config_mod/types.rs`
- Add GUI controls in `src/gui/settings.rs`
- Modify `apply_volume_to_packet` to handle balance

### 4. Improve Soft Clipping Algorithm

**Goals**:
- Replace simple soft clipping with a better algorithm
- Maintain audio quality when mixing loud signals

**Changes**:
- Implement a tanh-based soft clipper for better distortion characteristics
- Add oversampling to reduce aliasing

### 5. Add Audio Compression/Limiting

**Goals**:
- Add dynamic range compression to even out audio levels
- Add limiting to prevent clipping

**Changes**:
- Implement a simple compressor/limiter
- Add configuration options for compression settings
- Integrate compression into the mixing pipeline

### 6. Implement Audio Synchronization Mechanism

**Goals**:
- Ensure system and microphone audio are perfectly synchronized
- Handle any timestamp drift between capture devices

**Changes**:
- Implement a timestamp alignment algorithm
- Add buffering to handle latency differences
- Monitor and adjust synchronization during capture

### 7. Add Configuration Options for Audio Settings

**Goals**:
- Extend the configuration system to support all new audio settings
- Ensure backward compatibility with existing config files

**Changes**:
- Update `AudioConfig` in `src/config/config_mod/types.rs`
- Add validation for new fields
- Modify `requires_pipeline_restart` to include audio setting changes

### 8. Update GUI to Expose New Audio Controls

**Goals**:
- Add new audio controls to the settings UI
- Allow users to adjust mix levels, balance, and compression
- Provide real-time feedback on audio levels

**Changes**:
- Modify `src/gui/settings.rs` to add new controls
- Add volume meters for system and microphone
- Implement slider controls for balance and compression settings

### 9. Test Audio Quality and Synchronization

**Goals**:
- Verify audio quality after mixing
- Test synchronization between audio and video
- Ensure all new features work correctly

**Changes**:
- Create test cases for audio mixing
- Implement integration tests for the entire audio pipeline
- Add performance benchmarks

### 10. Optimize Performance of Audio Processing

**Goals**:
- Ensure audio processing doesn't impact capture performance
- Optimize mixing and effects processing

**Changes**:
- Profile audio processing code
- Optimize critical sections
- Consider SIMD optimization for audio calculations

## Files to Modify

1. `src/capture/audio/manager.rs` - Update WasapiAudioManager for mixing
2. `src/config/config_mod/types.rs` - Extend AudioConfig
3. `src/config/config_mod/audioconfig_traits.rs` - Add default values
4. `src/app/pipeline/audio.rs` - Update audio pipeline
5. `src/output/mp4.rs` - Simplify muxing (mixing now happens in capture)
6. `src/gui/settings.rs` - Add new audio controls
7. `src/encode/encoder_mod/types.rs` - Modify EncodedPacket if needed

## Benefits

1. **Better Audio Quality**: Improved mixing, compression, and clipping
2. **Real-time Control**: Adjust levels and balance during capture
3. **Synchronized Audio**: Prevent desynchronization issues
4. **Enhanced Features**: Compression, limiting, and better clipping
5. **User-Friendly**: New GUI controls for audio settings

## Risks and Mitigation

1. **Performance Impact**: Audio processing may affect capture performance. Mitigation: Optimize critical sections and test on low-end hardware.
2. **Complexity**: Increased code complexity. Mitigation: Keep audio processing modular and well-documented.
3. **Backward Compatibility**: New config fields may break existing installations. Mitigation: Ensure backward compatibility with existing config files.
