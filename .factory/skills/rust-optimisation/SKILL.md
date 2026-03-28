---
name: rust-optimisation
description: >-
  Optimise Rust code for performance, memory efficiency, and throughput. Use
  this skill whenever the user asks to optimise, profile, speed up, or reduce
  memory usage in Rust code. Also trigger when reviewing Rust code where
  performance matters — hot loops, real-time pipelines, ring buffers, lock-free
  structures, multimedia encoding/decoding, GPU interop, or systems-level code.
  Trigger on phrases like "make this faster", "reduce allocations",
  "zero-copy", "profiling", "benchmark", "flamegraph", "cache-friendly",
  "SIMD", or any mention of Rust performance work. If the user pastes Rust code
  and asks for a review or improvements, always consider performance as a
  dimension even if not explicitly requested.
---

# Rust Performance Optimisation Guide

You are a Rust performance engineer. When optimising code, follow this methodology: **measure first, hypothesise, change one thing, measure again**. Never optimise blindly.

## Core Methodology

### 1. Profile Before You Touch Anything

Never guess at bottlenecks. Use these tools in order of reach:

**Quick timing** — `std::time::Instant` for ad-hoc measurements:
```rust
let start = std::time::Instant::now();
// ... hot section ...
tracing::debug!("section took {:?}", start.elapsed());
```

**Criterion benchmarks** — for micro-benchmarks with statistical rigour:
```rust
// benches/my_bench.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_ring_buffer_write(c: &mut Criterion) {
    c.bench_function("ring_write_1kb", |b| {
        let mut ring = RingBuffer::new(1024 * 1024);
        let data = vec![0u8; 1024];
        b.iter(|| ring.push(criterion::black_box(&data)));
    });
}
criterion_group!(benches, bench_ring_buffer_write);
criterion_main!(benches);
```

**Flamegraphs** — for finding where wall-clock time actually goes:
```bash
# Using cargo-flamegraph (wraps perf on Linux, DTrace on macOS)
cargo flamegraph --bin my_app -- --some-flag

# Or with samply for a more interactive viewer
cargo install samply
samply record ./target/release/my_app
```

**Heap profiling** — for tracking allocations:
```bash
# DHAT (built into Rust via dhat crate)
# Add to Cargo.toml: dhat = "0.3"
# Add at program start:
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

# Or use heaptrack on Linux
heaptrack ./target/release/my_app
```

**Windows-specific profiling**:
- ETW traces via `xperf` / Windows Performance Recorder
- Superluminal for sampling + instrumentation
- Tracy (via `tracing-tracy` crate) for frame-level profiling — excellent for real-time apps
- `tracelog` + WPA for GPU and DXGI analysis

### 2. Know Your Optimisation Targets

Establish what "fast enough" means before starting. Common targets:

| Domain | Typical Target | Key Metric |
|--------|---------------|------------|
| Real-time video (30fps) | <33ms per frame | Frame time p99 |
| Real-time video (60fps) | <16ms per frame | Frame time p99 |
| Audio processing | <5ms per callback | Callback duration |
| Ring buffer write | <1μs per packet | Write latency p99 |
| UI responsiveness | <16ms per frame | Event-to-paint |
| CLI tool startup | <100ms | Time to first output |

---

## Allocation & Memory Patterns

Allocations are the single most common performance problem in Rust. The allocator is not free — each `malloc`/`free` involves locking, bookkeeping, and potential syscalls.

### Eliminate Unnecessary Allocations

**Reuse buffers instead of reallocating**:
```rust
// BAD — allocates every iteration
for packet in packets {
    let mut buf = Vec::new();
    encode_into(&mut buf, packet);
    send(buf);
}

// GOOD — reuse the buffer
let mut buf = Vec::with_capacity(expected_size);
for packet in packets {
    buf.clear(); // len = 0, capacity preserved
    encode_into(&mut buf, packet);
    send(&buf);
}
```

**Use `SmallVec` for small, bounded collections**:
```rust
use smallvec::SmallVec;

// Stack-allocated for ≤8 elements, heap-allocated beyond
let mut tags: SmallVec<[Tag; 8]> = SmallVec::new();
```

**Avoid `format!()` in hot paths** — it allocates a `String` every time:
```rust
// BAD in a hot loop
log::debug!("frame {}: pts={}", frame_num, pts);

// GOOD — use tracing with zero-alloc structured fields
tracing::debug!(frame_num, pts, "processing frame");
```

**Prefer `&str` over `String`, `&[u8]` over `Vec<u8>`** when you don't need ownership. If you need cheap cloning of byte buffers, use `bytes::Bytes` which is reference-counted:
```rust
use bytes::Bytes;

// Bytes::clone() is an Arc bump — O(1), no copy
let snapshot = buffer.clone(); // cheap
```

### Pre-size Collections

```rust
// BAD — grows and reallocates multiple times
let mut results = Vec::new();
for item in source {
    results.push(process(item));
}

// GOOD — single allocation
let mut results = Vec::with_capacity(source.len());
for item in source {
    results.push(process(item));
}

// BEST — iterator chain, compiler can optimise further
let results: Vec<_> = source.iter().map(process).collect();
// collect() uses size_hint() internally to pre-allocate
```

### Avoid Clone When Borrow Will Do

```rust
// BAD — clones the entire string
fn process(data: String) { /* ... */ }
process(my_string.clone());

// GOOD — borrow instead
fn process(data: &str) { /* ... */ }
process(&my_string);
```

When you genuinely need shared ownership, prefer `Arc<T>` over cloning large data. For shared byte buffers, `bytes::Bytes` is purpose-built.

---

## Zero-Copy Patterns

Zero-copy means processing data without copying it between pipeline stages. Critical for video/audio pipelines where frames can be megabytes.

### Slicing Without Copying

```rust
use bytes::Bytes;

let full_buffer: Bytes = capture_frame();

// .slice() shares the underlying allocation — no copy
let header = full_buffer.slice(0..64);
let payload = full_buffer.slice(64..);
```

### Memory-Mapped I/O

```rust
use memmap2::MmapOptions;
use std::fs::File;

let file = File::open("large_file.bin")?;
let mmap = unsafe { MmapOptions::new().map(&file)? };

// Access file contents as &[u8] — OS handles paging
let header = &mmap[0..64];
```

### GPU Texture Sharing (D3D11/Vulkan)

For video pipelines, avoid GPU→CPU→GPU round-trips:
```
Capture (ID3D11Texture2D)
    → Share via same ID3D11Device
        → Encoder reads directly (NVENC/AMF/QSV)
            → No CPU readback in the hot path
```

Keep textures on the GPU. Only read back to CPU when absolutely necessary (e.g., software encoder fallback or thumbnail generation).

---

## Lock-Free & Concurrent Patterns

### Choose the Right Synchronisation Primitive

| Need | Use | Avoid |
|------|-----|-------|
| Single flag | `AtomicBool` | `Mutex<bool>` |
| Shared counter | `AtomicUsize` | `Mutex<usize>` |
| SPSC queue | `crossbeam::channel::bounded` or `ringbuf` | `Mutex<VecDeque>` |
| MPSC queue | `crossbeam::channel::bounded` | `std::sync::mpsc` (unbounded = OOM risk) |
| Shared config | `arc_swap::ArcSwap` | `RwLock<Config>` |
| Read-heavy shared data | `parking_lot::RwLock` | `std::sync::RwLock` |

### Bounded Channels for Backpressure

Always prefer bounded channels in real-time pipelines. Unbounded channels mask backpressure and lead to memory growth:

```rust
use crossbeam::channel;

// GOOD — bounded channel applies natural backpressure
let (tx, rx) = channel::bounded::<EncodedPacket>(12);

// Producer blocks when channel is full — this is correct behaviour.
// It means the consumer can't keep up and the system self-regulates.
tx.send(packet)?;

// BAD — unbounded means unlimited memory growth if consumer is slow
let (tx, rx) = channel::unbounded();
```

### Atomic Ordering Guide

```rust
use std::sync::atomic::Ordering;

// For simple flags (stop signals, feature toggles):
flag.store(true, Ordering::Relaxed);

// For publish/subscribe patterns (one thread writes data, another reads):
// Writer: Release after writing the data
data.store(value, Ordering::Release);
// Reader: Acquire before reading the data
let val = data.load(Ordering::Acquire);

// If in doubt, use SeqCst — it's slower but always correct
counter.fetch_add(1, Ordering::SeqCst);
```

### Reduce Lock Contention

```rust
// BAD — lock held across I/O
{
    let mut state = state.lock();
    state.buffer.extend_from_slice(&data);
    state.flush_to_disk()?; // blocks while locked!
}

// GOOD — lock only for the data mutation, I/O outside
let flush_data = {
    let mut state = state.lock();
    state.buffer.extend_from_slice(&data);
    state.buffer.clone() // or swap with empty buffer
};
flush_to_disk(&flush_data)?;
```

---

## Iterator & Loop Optimisation

### Prefer Iterators Over Manual Indexing

Rust's iterator chains compile to the same (or better) machine code as manual loops, and they enable bounds-check elimination:

```rust
// This eliminates bounds checks — the compiler knows the length
let sum: u64 = data.iter().map(|x| x.value as u64).sum();

// Manual indexing may retain bounds checks
let mut sum = 0u64;
for i in 0..data.len() {
    sum += data[i].value as u64; // bounds check on each access
}
```

### Avoid Unnecessary Work in Hot Loops

```rust
// BAD — recomputes divisor every iteration
for sample in &samples {
    let normalised = sample.value / samples.len() as f64;
}

// GOOD — hoist the invariant
let inv_len = 1.0 / samples.len() as f64;
for sample in &samples {
    let normalised = sample.value * inv_len;
}
```

### Use `chunks_exact()` for SIMD-Friendly Patterns

```rust
// Process 4 elements at a time — compiler can auto-vectorise
for chunk in data.chunks_exact(4) {
    let a = chunk[0] + chunk[1];
    let b = chunk[2] + chunk[3];
    results.push(a + b);
}
// Handle remainder
let remainder = data.chunks_exact(4).remainder();
```

---

## String & Formatting Performance

### Avoid Allocations in String Building

```rust
use std::fmt::Write;

// BAD — allocates on every format!()
let mut output = String::new();
for item in items {
    output += &format!("{}: {}\n", item.name, item.value);
}

// GOOD — write! into a pre-allocated String
let mut output = String::with_capacity(items.len() * 64);
for item in items {
    write!(output, "{}: {}\n", item.name, item.value).unwrap();
}
```

### Use `Cow<str>` for Conditional Ownership

```rust
use std::borrow::Cow;

fn normalise_path(path: &str) -> Cow<str> {
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path) // no allocation
    }
}
```

---

## Compiler & Build Configuration

### Release Profile Tuning

```toml
# Cargo.toml
[profile.release]
opt-level = 3          # max optimisation
lto = "fat"            # cross-crate inlining — slower builds, faster binary
codegen-units = 1      # single codegen unit — better optimisation, slower build
panic = "abort"        # smaller binary, no unwinding overhead
strip = true           # strip debug symbols from release binary

[profile.release.package."*"]
opt-level = 3          # also optimise dependencies

# Dev profile — keep debug usable but not glacially slow
[profile.dev]
opt-level = 1          # basic optimisation so dev builds aren't unusably slow
```

### Target-Specific Optimisation

```toml
# .cargo/config.toml

# Enable native CPU features (AVX2, etc.) — binary only runs on similar CPUs
[build]
rustflags = ["-C", "target-cpu=native"]
```

For distributed binaries, use feature detection at runtime instead:
```rust
if is_x86_feature_detected!("avx2") {
    process_avx2(&data);
} else {
    process_fallback(&data);
}
```

### Compile Time Improvements

```toml
# Faster dev builds at the cost of runtime speed
[profile.dev]
opt-level = 1
debug = 2

[profile.dev.package."*"]
opt-level = 2  # optimise deps even in dev — they rarely change
```

Use `cargo build --timings` to find slow crates. Consider replacing heavy compile-time dependencies:
- `serde_json` → `simd-json` or `sonic-rs` (if parsing is on the hot path)
- `regex` → `aho-corasick` or literal matching (if patterns are simple)
- `reqwest` → `ureq` (if you don't need async HTTP)

---

## Data Structure & Layout

### Struct Layout for Cache Efficiency

```rust
// BAD — bool fields cause padding waste
struct Packet {
    is_keyframe: bool,    // 1 byte + 7 padding
    timestamp: u64,       // 8 bytes
    is_audio: bool,       // 1 byte + 7 padding
    size: u64,            // 8 bytes
}
// Total: 32 bytes (16 bytes wasted on padding)

// GOOD — group by size, bools together
struct Packet {
    timestamp: u64,       // 8 bytes
    size: u64,            // 8 bytes
    is_keyframe: bool,    // 1 byte
    is_audio: bool,       // 1 byte + 6 padding
}
// Total: 24 bytes

// Or use repr(C) + manual packing, or bitflags for bools
```

### SoA vs AoS

For batch processing, Structure of Arrays often outperforms Array of Structures:

```rust
// AoS — poor cache utilisation if you only read timestamps
struct Packet { timestamp: u64, data: [u8; 1024] }
let packets: Vec<Packet> = ...;

// SoA — timestamps are contiguous in memory
struct PacketStore {
    timestamps: Vec<u64>,
    data: Vec<[u8; 1024]>,
}
// Iterating timestamps only touches timestamp cache lines
```

### Use `Box<[T]>` Over `Vec<T>` for Fixed-Size Buffers

```rust
// Vec carries capacity + length + pointer (24 bytes on stack)
// Box<[T]> carries pointer + length only (16 bytes) and can't grow
let fixed: Box<[u8]> = vec![0u8; 4096].into_boxed_slice();
```

---

## Async-Specific Optimisation

### Keep Futures Small

Large futures bloat the task size and slow down the executor:

```rust
// BAD — large array lives in the future's state machine
async fn process() {
    let buffer = [0u8; 65536]; // 64KB in the future!
    do_work(&buffer).await;
}

// GOOD — heap-allocate large buffers
async fn process() {
    let buffer = vec![0u8; 65536]; // on the heap, future stays small
    do_work(&buffer).await;
}
```

### Avoid Holding Locks Across `.await`

```rust
// BAD — MutexGuard held across await point = blocks executor thread
let guard = mutex.lock().await;
some_io().await; // other tasks can't progress on this thread
drop(guard);

// GOOD — take what you need, drop the guard, then await
let data = {
    let guard = mutex.lock().await;
    guard.clone()
};
some_io_with(data).await;
```

### Prefer `tokio::task::spawn_blocking` for CPU Work

```rust
// Don't block the async runtime with CPU-intensive work
let result = tokio::task::spawn_blocking(move || {
    expensive_computation(&data)
}).await?;
```

---

## Common Pitfalls Checklist

When reviewing Rust code for performance, check for these frequent issues:

- [ ] **Unnecessary `.clone()`** — Can it be a borrow instead?
- [ ] **Unbounded channels or collections** — Should they be bounded?
- [ ] **`format!()` in hot paths** — Use `write!()` or structured logging
- [ ] **Lock held across I/O or `.await`** — Minimise critical sections
- [ ] **`Vec` growing repeatedly** — Pre-allocate with `with_capacity()`
- [ ] **Debug assertions / logging in release** — Gate with `#[cfg(debug_assertions)]`
- [ ] **String concatenation with `+`** — Use `String::push_str()` or `write!()`
- [ ] **Redundant `.to_string()` / `.to_owned()`** — Borrow where possible
- [ ] **`HashMap` with small key sets** — Consider a sorted `Vec` or `phf`
- [ ] **Per-frame Win32 API calls** — Set styles once, not every tick
- [ ] **Dropping large allocations on the main thread** — Move drops to a background thread if latency-sensitive
- [ ] **`Box<dyn Trait>` in hot paths** — Virtual dispatch + heap allocation; consider enums or generics

---

## When to Optimise vs When to Ship

Not everything needs to be fast. Apply this decision framework:

1. **Is it in the hot path?** If code runs once at startup or on a button click, don't optimise it. Focus on code that runs per-frame, per-packet, or per-request.
2. **Is it measurably slow?** If you can't show it's slow with a profiler, it's not slow. Intuition about performance is frequently wrong.
3. **Will users notice?** A 2ms improvement in a 500ms operation is irrelevant. A 2ms improvement in a 16ms frame budget is significant.
4. **Does it sacrifice readability?** Clever optimisations that nobody can maintain are a net negative. Prefer clear code with good algorithms over micro-optimised spaghetti.

When in doubt, write clear code first, benchmark it, and only optimise if the numbers demand it.
