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

**Statistical timing with histograms** — for production metrics:
```rust
use hdrhistogram::Histogram;

let mut hist = Histogram::new(3).unwrap(); // 3 significant digits
for _ in 0..1000 {
    let start = std::time::Instant::now();
    do_work();
    hist.record(start.elapsed().as_nanos() as u64).unwrap();
}
println!("p50={:?} p99={:?} p99.9={:?}", 
    std::time::Duration::from_nanos(hist.value_at_percentile(50.0)),
    std::time::Duration::from_nanos(hist.value_at_percentile(99.0)),
    std::time::Duration::from_nanos(hist.value_at_percentile(99.9)));
```

**Criterion benchmarks** — for micro-benchmarks with statistical rigour:
```rust
// benches/my_bench.rs
use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn bench_ring_buffer_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("ring_buffer");
    group.throughput(Throughput::Bytes(1024)); // report bytes/sec
    group.bench_function("write_1kb", |b| {
        let mut ring = RingBuffer::new(1024 * 1024);
        let data = vec![0u8; 1024];
        b.iter(|| ring.push(criterion::black_box(&data)));
    });
    group.finish();
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

# For long-running services, attach to running process
samply record -p <pid>
```

**Heap profiling** — for tracking allocations:
```bash
# DHAT (built into Rust via dhat crate)
# Add to Cargo.toml: dhat = "0.3"
# Add at program start:
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

# Run program, then view DHAT output
# Shows allocation sites, lifetimes, leak suspects

# Or use heaptrack on Linux
heaptrack ./target/release/my_app
heaptrack_print heaptrack.<pid>.gz
```

**Memory sanitizers** — for detecting undefined behavior that affects performance:
```bash
# AddressSanitizer — detects heap/stack buffer overflows, use-after-free
RUSTFLAGS="-Zsanitizer=address" cargo run --target x86_64-unknown-linux-gnu

# MemorySanitizer — detects use of uninitialized memory
RUSTFLAGS="-Zsanitizer=memory" cargo run --target x86_64-unknown-linux-gnu

# ThreadSanitizer — detects data races
RUSTFLAGS="-Zsanitizer=thread" cargo run --target x86_64-unknown-linux-gnu
```

**Windows-specific profiling**:
- **ETW (Event Tracing for Windows)** via `xperf` / Windows Performance Recorder / Windows Performance Analyzer (WPA)
- **Superluminal** for sampling + instrumentation with excellent timeline visualization
- **Tracy** (via `tracing-tracy` crate) for frame-level profiling — excellent for real-time apps
- **Windows Performance Toolkit (WPT)** for GPU and DXGI analysis, CPU sampling, context switches
- **GPUView** for detailed GPU timing and synchronization
- **PIX for Windows** for DirectX debugging and frame analysis
```bash
# Quick ETW trace capture
xperf -on base+latency+power -start GPU -on "GPU"
# Run your app
xperf -stop -d trace.etl
# Open in WPA
wpa trace.etl
```

**Linux-specific profiling**:
```bash
# perf for CPU profiling
perf record -g ./target/release/my_app
perf report

# perf stat for hardware counters
perf stat -e cycles,instructions,cache-misses,cache-references ./target/release/my_app

# eBPF for kernel-level tracing
bpftrace -e 'profile:hz:99 /pid == <pid>/ { @[ustack] = count(); }'
```

**macOS-specific profiling**:
```bash
# Instruments (Xcode)
instruments -t "Time Profiler" ./target/release/my_app

# Sample tool for quick profiling
sample <pid> 10  # sample for 10 seconds

# dtrace for low-overhead tracing
sudo dtrace -n 'profile-997 /pid == <pid>/ { @[ustack(10)] = count(); }'
```

### 2. Know Your Optimisation Targets

Establish what "fast enough" means before starting. Common targets:

| Domain | Typical Target | Key Metric |
|--------|---------------|------------|
| Real-time video (30fps) | <33ms per frame | Frame time p99 |
| Real-time video (60fps) | <16ms per frame | Frame time p99 |
| Real-time video (120fps) | <8ms per frame | Frame time p99 |
| Audio processing | <5ms per callback | Callback duration |
| Ring buffer write | <1μs per packet | Write latency p99 |
| UI responsiveness | <16ms per frame | Event-to-paint |
| CLI tool startup | <100ms | Time to first output |
| Web API request | <50ms p99 | Request latency |
| Game loop iteration | <16ms | Frame time consistency |

### 3. Performance Testing Methodology

**Micro-benchmarks vs Real-world benchmarks**:
- Micro-benchmarks (Criterion) are great for isolated functions but can miss cache effects and interactions
- Real-world benchmarks test the full system but are harder to isolate
- Do both: micro-benchmarks for regression testing, real-world for validation

**Statistical significance**:
```rust
// Criterion automatically handles this, but for manual testing:
// Run at least 100 iterations, discard outliers, report percentiles
// Never report just the mean — p50/p90/p99 tell the real story
```

**Regression testing in CI**:
```yaml
# .github/workflows/bench.yml
- name: Run benchmarks
  run: cargo bench -- --save-baseline main

- name: Compare with main
  run: cargo bench -- --baseline main
  # Fails if performance regresses by >10%
```

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

// BEST — use BytesMut for zero-copy handoff
use bytes::BytesMut;
let mut buf = BytesMut::with_capacity(expected_size);
for packet in packets {
    buf.clear();
    encode_into(&mut buf, packet);
    send(buf.split().freeze()); // zero-copy transfer
}
```

**Use `SmallVec` for small, bounded collections**:
```rust
use smallvec::SmallVec;

// Stack-allocated for ≤8 elements, heap-allocated beyond
let mut tags: SmallVec<[Tag; 8]> = SmallVec::new();

// Also useful: SmallVec<[u8; 64]> for small byte buffers
// Avoids heap allocation for the common case
```

**Use `ArrayVec` for fixed-capacity stack buffers**:
```rust
use arrayvec::ArrayVec;

// Never heap-allocates, fixed capacity
let mut buf: ArrayVec<u8, 64> = ArrayVec::new();
buf.try_extend_from_slice(&data).ok(); // fails if data > 64 bytes
```

**Avoid `format!()` in hot paths** — it allocates a `String` every time:
```rust
// BAD in a hot loop
log::debug!("frame {}: pts={}", frame_num, pts);

// GOOD — use tracing with zero-alloc structured fields
tracing::debug!(frame_num, pts, "processing frame");

// For non-structured output, use write! into a reused buffer
use std::fmt::Write;
let mut buf = String::with_capacity(64);
write!(buf, "frame {}: pts={}", frame_num, pts).unwrap();
// use buf...
buf.clear(); // reuse
```

**Prefer `&str` over `String`, `&[u8]` over `Vec<u8>`** when you don't need ownership. If you need cheap cloning of byte buffers, use `bytes::Bytes` which is reference-counted:
```rust
use bytes::Bytes;

// Bytes::clone() is an Arc bump — O(1), no copy
let snapshot = buffer.clone(); // cheap

// Bytes also supports slicing without copy
let header = snapshot.slice(0..64);
let body = snapshot.slice(64..);
// Both share the same underlying allocation
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

// For HashMap, pre-size based on expected entries
let mut map = HashMap::with_capacity(expected_keys);
// HashMap also has with_hasher for custom hashers
```

### Avoid Clone When Borrow Will Do

```rust
// BAD — clones the entire string
fn process(data: String) { /* ... */ }
process(my_string.clone());

// GOOD — borrow instead
fn process(data: &str) { /* ... */ }
process(&my_string);

// When you need owned output, consider Cow
use std::borrow::Cow;
fn process<'a>(data: &'a str) -> Cow<'a, str> {
    if needs_modification(data) {
        Cow::Owned(modify(data))
    } else {
        Cow::Borrowed(data) // no allocation
    }
}
```

When you genuinely need shared ownership, prefer `Arc<T>` over cloning large data. For shared byte buffers, `bytes::Bytes` is purpose-built.

### Object Pooling

For frequently allocated/deallocated objects, use pooling:

```rust
// Simple object pool using Vec recycling
struct Pool<T> {
    items: Vec<T>,
    created: usize,
}

impl<T> Pool<T> {
    fn get(&mut self) -> T {
        self.items.pop().unwrap_or_else(|| {
            self.created += 1;
            T::default()
        })
    }
    
    fn return_item(&mut self, item: T) {
        self.items.push(item);
    }
}

// Or use the `object-pool` or `typed-arena` crates
use typed_arena::Arena;

let arena = Arena::new();
for i in 0..1000 {
    let obj = arena.alloc(MyStruct::new(i)); // all on same arena
}
// All allocations freed at once when arena is dropped
```

### Memory Allocator Selection

The default system allocator is general-purpose. For specific workloads, consider alternatives:

```rust
// jemalloc — often faster for multi-threaded workloads
// Cargo.toml: tikv-jemallocator = { version = "0.5", features = ["unprefixed_malloc_on_supported_platforms"] }
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// mimalloc — Microsoft's allocator, good for Windows
// Cargo.toml: mimalloc = "0.1"
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

// For custom allocation tracking
#[global_allocator]
static ALLOC: TrackingAlloc = TrackingAlloc::new(std::alloc::System);
```

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

// For mutable slicing, use BytesMut
use bytes::BytesMut;
let mut buf = BytesMut::from(&data[..]);
let header = buf.split_to(64); // no copy, just pointer adjustment
```

### Memory-Mapped I/O

```rust
use memmap2::MmapOptions;
use std::fs::File;

let file = File::open("large_file.bin")?;
let mmap = unsafe { MmapOptions::new().map(&file)? };

// Access file contents as &[u8] — OS handles paging
let header = &mmap[0..64];

// For writing, use map_mut()
let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };
mmap[0] = 0xFF;
mmap.flush()?; // sync to disk
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

```rust
// D3D11 texture sharing pattern
// Create texture with D3D11_RESOURCE_MISC_SHARED
let desc = D3D11_TEXTURE2D_DESC {
    MiscFlags: D3D11_RESOURCE_MISC_SHARED,
    // ...
};

// Share across devices via OpenSharedResource
// Or use same device for capture and encode
```

### Send + Sync for Zero-Copy Sharing

```rust
// For zero-copy sharing across threads, data must be Send + Sync
// Bytes is Send + Sync because it uses Arc internally
let data: Bytes = capture_frame();
std::thread::spawn(move || {
    process(data); // works because Bytes: Send
});

// For custom types, consider:
// - Arc<T> where T: Send + Sync
// - Arc<[u8]> for byte slices
// - &'static [u8] for static data (include_bytes!)
```

---

## Lock-Free & Concurrent Patterns

### Choose the Right Synchronisation Primitive

| Need | Use | Avoid |
|------|-----|-------|
| Single flag | `AtomicBool` | `Mutex<bool>` |
| Shared counter | `AtomicUsize` | `Mutex<usize>` |
| SPSC queue | `crossbeam::channel::bounded` or `ringbuf` | `Mutex<VecDeque>` |
| MPSC queue | `crossbeam::channel::bounded` | `std::sync::mpsc` (unbounded = OOM risk) |
| SPMC queue | `crossbeam::deque` or custom ring buffer | `Mutex<Vec>` with readers |
| MPMC queue | `crossbeam::channel::bounded` | Multiple Mutex queues |
| Shared config | `arc_swap::ArcSwap` | `RwLock<Config>` |
| Read-heavy shared data | `parking_lot::RwLock` | `std::sync::RwLock` |
| Write-heavy shared data | `Mutex` or lock-free structure | `RwLock` (writers block readers) |
| Once initialization | `std::sync::OnceLock` or `lazy_static` | `Option<T>` with Mutex |

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

// For non-blocking send with backpressure awareness
match tx.try_send(packet) {
    Ok(()) => {},
    Err(crossbeam::channel::TrySendError::Full(_)) => {
        // Handle backpressure — drop frame, signal throttle, etc.
    }
    Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
        // Consumer closed
    }
}
```

### Lock-Free Ring Buffer (SPMC)

For single-producer multiple-consumer scenarios (like replay buffers):

```rust
// Key insight: single producer means no synchronization on write
// Multiple readers use atomic indices to track their position

struct RingBuffer<T> {
    data: Box<[UnsafeCell<T>]>,
    capacity: usize,
    head: AtomicUsize, // write position (producer only)
    // Each reader tracks its own tail position
}

// Producer writes without locks
fn push(&self, item: T) {
    let pos = self.head.load(Ordering::Relaxed);
    let idx = pos % self.capacity;
    unsafe { *self.data[idx].get() = item; }
    self.head.store(pos + 1, Ordering::Release);
}

// Reader reads with acquire ordering
fn read(&self, reader_pos: usize) -> Option<&T> {
    let head = self.head.load(Ordering::Acquire);
    if reader_pos >= head { return None; }
    let idx = reader_pos % self.capacity;
    Some(unsafe { &*self.data[idx].get() })
}
```

### Atomic Ordering Guide

```rust
use std::sync::atomic::Ordering;

// For simple flags (stop signals, feature toggles):
// Relaxed is fine — no data being synchronized, just the flag itself
flag.store(true, Ordering::Relaxed);
if flag.load(Ordering::Relaxed) { /* ... */ }

// For publish/subscribe patterns (one thread writes data, another reads):
// Writer: Release after writing the data
data.store(value, Ordering::Release); // "publish" the data
// Reader: Acquire before reading the data
let val = data.load(Ordering::Acquire); // "subscribe" to the data

// For counters that need precise ordering:
counter.fetch_add(1, Ordering::SeqCst);

// For compare-and-swap loops:
loop {
    let current = shared.load(Ordering::Acquire);
    let new = compute(current);
    match shared.compare_exchange_weak(
        current, new, Ordering::Release, Ordering::Relaxed
    ) {
        Ok(_) => break,
        Err(_) => continue, // retry
    }
}

// Ordering cheat sheet:
// - Relaxed: No synchronization, just atomicity
// - Acquire: No reads/writes before the load can be reordered after it
// - Release: No reads/writes after the store can be reordered before it
// - AcqRel: Acquire + Release (for read-modify-write operations)
// - SeqCst: Total order, slowest but always correct
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

// BETTER — use swap for zero-copy extraction
let flush_data = {
    let mut state = state.lock();
    std::mem::take(&mut state.buffer) // replace with empty, return old
};
flush_to_disk(&flush_data)?;
```

### Sharding for High Contention

For highly contended data, shard into multiple locks:

```rust
use std::sync::Mutex;

struct ShardedCounter {
    shards: [Mutex<u64>; 16], // 16 shards
}

impl ShardedCounter {
    fn increment(&self, key: u64) {
        let shard_idx = (key % 16) as usize;
        *self.shards[shard_idx].lock().unwrap() += 1;
    }
    
    fn total(&self) -> u64 {
        self.shards.iter().map(|s| *s.lock().unwrap()).sum()
    }
}
// Reduces contention by 16x for uniform key distribution
```

---

## Cache Optimisation

### Cache Line Awareness

Modern CPUs have 64-byte cache lines. Data accessed together should be on the same cache line; data accessed independently should be on different cache lines.

```rust
// BAD — hot and cold data on same cache line
struct Packet {
    timestamp: u64,      // hot - accessed every frame
    flags: u32,          // hot
    debug_info: [u8; 48], // cold - only used for debugging
}

// GOOD — separate hot and cold
#[repr(align(64))]
struct HotData {
    timestamp: u64,
    flags: u32,
    // padding to 64 bytes happens automatically
}

struct Packet {
    hot: HotData,        // one cache line
    debug_info: Box<[u8]>, // separate allocation
}
```

### False Sharing Prevention

When multiple threads write to different variables on the same cache line, they cause cache thrashing:

```rust
use std::cell::UnsafeCell;

// BAD — counters on same cache line cause false sharing
struct Counters {
    packets: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
}

// GOOD — each counter on its own cache line
#[repr(align(64))]
struct CacheAligned<T>(T);

struct Counters {
    packets: CacheAligned<AtomicU64>,
    bytes: CacheAligned<AtomicU64>,
    errors: CacheAligned<AtomicU64>,
}
// Now each counter is on its own cache line, no false sharing
```

### Prefetching

For predictable access patterns, prefetching can hide memory latency:

```rust
use std::intrinsics::prefetch::{prefetch_read_data, PREFETCH_LOCALITY3};

// Prefetch data you'll need soon
for i in 0..data.len() {
    // Prefetch 8 elements ahead
    if i + 8 < data.len() {
        unsafe {
            prefetch_read_data(&data[i + 8], PREFETCH_LOCALITY3);
        }
    }
    process(&data[i]);
}

// Or use the portable hint module
use std::hint::prefetch;
for i in 0..data.len() {
    if i + 8 < data.len() {
        std::hint::prefetch(std::hint::Prefetch::Read, &data[i + 8]);
    }
    process(&data[i]);
}
```

### Cache-Friendly Algorithms

```rust
// BAD — pointer-chasing (linked list)
for node in list.iter() {
    process(node.data); // each node is a cache miss
}

// GOOD — contiguous iteration (Vec)
for item in vec.iter() {
    process(item); // sequential access, prefetcher kicks in
}

// For lookups, consider:
// - Sorted Vec + binary search for small datasets (<1000 elements)
// - HashMap for larger datasets
// - B-tree for range queries
```

---

## SIMD & Vectorization

### Auto-Vectorization

The compiler can auto-vectorize simple loops. Help it by:

```rust
// 1. Use simple iterator patterns
let sum: i32 = data.iter().map(|x| x * 2).sum();
// Compiler can vectorize this

// 2. Avoid early exits in hot loops
// BAD
for x in &data {
    if *x > threshold { break; } // prevents vectorization
    process(x);
}
// GOOD — process all, filter after
let results: Vec<_> = data.iter()
    .take_while(|x| **x <= threshold)
    .map(process)
    .collect();

// 3. Use assert to enable bounds check elimination
fn process(data: &[u8]) {
    assert!(data.len() % 4 == 0);
    for chunk in data.chunks_exact(4) {
        // Compiler knows chunk.len() == 4, can vectorize
    }
}
```

### Explicit SIMD with Portable SIMD

```rust
#![feature(portable_simd)]
use std::simd::*;

fn add_arrays_simd(a: &[f32], b: &[f32], out: &mut [f32]) {
    assert!(a.len() == b.len() && b.len() == out.len());
    
    let chunks = a.len() / 8; // Process 8 floats at a time (AVX)
    
    for i in 0..chunks {
        let idx = i * 8;
        let va = f32x8::from_slice(&a[idx..idx+8]);
        let vb = f32x8::from_slice(&b[idx..idx+8]);
        out[idx..idx+8].copy_from_slice(&(va + vb).to_array());
    }
    
    // Handle remainder
    for i in (chunks * 8)..a.len() {
        out[i] = a[i] + b[i];
    }
}
```

### SIMD with `packed_simd_2` or `wide`

```rust
// Using the `wide` crate (stable Rust)
use wide::f32x8;

fn add_arrays_wide(a: &[f32], b: &[f32], out: &mut [f32]) {
    for (chunk, (a_chunk, b_chunk)) in out.chunks_exact_mut(8)
        .zip(a.chunks_exact(8).zip(b.chunks_exact(8)))
    {
        let va = f32x8::from(a_chunk);
        let vb = f32x8::from(b_chunk);
        chunk.copy_from_slice(&(va + vb).as_array());
    }
    // Handle remainder...
}
```

### Runtime SIMD Dispatch

```rust
// Use feature detection to pick the best implementation
fn add_arrays(a: &[f32], b: &[f32], out: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { add_arrays_avx2(a, b, out) };
        }
    }
    add_arrays_fallback(a, b, out);
}

#[target_feature(enable = "avx2")]
unsafe fn add_arrays_avx2(a: &[f32], b: &[f32], out: &mut [f32]) {
    // AVX2 implementation
}

fn add_arrays_fallback(a: &[f32], b: &[f32], out: &mut [f32]) {
    for i in 0..a.len() {
        out[i] = a[i] + b[i];
    }
}
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

// Iterator chains also enable fusion
let result: Vec<_> = data.iter()
    .filter(|x| x.valid)
    .map(|x| x.value * 2)
    .collect();
// May compile to a single pass, not filter-then-map
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
    let normalised = sample.value * inv_len; // multiply faster than divide
}

// Also hoist function calls that return constants
let threshold = compute_threshold(); // once
for item in &items {
    if item.value > threshold { /* ... */ }
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

### Loop Unrolling

```rust
// Manual unrolling (sometimes helpful, often not needed)
for chunk in data.chunks_exact(4) {
    process(chunk[0]);
    process(chunk[1]);
    process(chunk[2]);
    process(chunk[3]);
}

// Or use unrolled iterator
use itertools::Itertools;
for (a, b, c, d) in data.iter().tuples() {
    process4(a, b, c, d);
}
```

### Branch Prediction Optimisation

```rust
// Sort data to improve branch prediction
// Branches are predictable when they follow patterns

// BAD — random branches
for item in &items {
    if item.is_valid() { // random order = branch mispredictions
        process(item);
    }
}

// GOOD — sorted by branch condition
let mut sorted: Vec<_> = items.iter().collect();
sorted.sort_by_key(|item| !item.is_valid()); // valid first
for item in &sorted {
    if item.is_valid() {
        process(item); // predictable branch
    }
}

// Or use branchless techniques
let mask = (value >= threshold) as usize; // 0 or 1
result += mask * contribution; // no branch
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

// For repeated appends, use push_str
output.push_str("prefix");
output.push_str(&item.name);
output.push_str("\n");
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

// Chain Cow operations efficiently
fn process(path: &str) -> Cow<str> {
    normalise_path(path)
        .into_owned()
        .into() // only allocate if needed
}
```

### Interning for Repeated Strings

```rust
use string_interner::{StringInterner, Symbol};

let mut interner = StringInterner::new();

// Store each unique string once
let sym1 = interner.get_or_intern("hello");
let sym2 = interner.get_or_intern("world");
let sym3 = interner.get_or_intern("hello"); // reuses sym1

// Compare symbols instead of strings (usize comparison)
assert_eq!(sym1, sym3);
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
debug = 1              # keep line info for profiling (optional)

[profile.release.package."*"]
opt-level = 3          # also optimise dependencies

# Dev profile — keep debug usable but not glacially slow
[profile.dev]
opt-level = 1          # basic optimisation so dev builds aren't unusably slow
debug = 2              # full debug info

# Release-with-debug for profiling
[profile.release-debug]
inherits = "release"
debug = 2              # full debug symbols for profiling
strip = false
```

### Target-Specific Optimisation

```toml
# .cargo/config.toml

# Enable native CPU features (AVX2, etc.) — binary only runs on similar CPUs
[build]
rustflags = ["-C", "target-cpu=native"]

# Or target-specific features
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "target-feature=+avx2,+fma"]

# For size optimization
[profile.release]
opt-level = "z"        # optimize for size
lto = true
codegen-units = 1
panic = "abort"
strip = true
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

# Cranelift backend for faster dev builds (unstable)
# .cargo/config.toml
[unstable]
codegen-backend = true

[profile.dev]
codegen-backend = "cranelift"
```

Use `cargo build --timings` to find slow crates. Consider replacing heavy compile-time dependencies:
- `serde_json` → `simd-json` or `sonic-rs` (if parsing is on the hot path)
- `regex` → `aho-corasick` or literal matching (if patterns are simple)
- `reqwest` → `ureq` (if you don't need async HTTP)
- `clap` → `pico-args` (for simple CLI)

### Link-Time Optimisation (LTO)

```toml
# Fat LTO — slowest build, fastest binary
[profile.release]
lto = "fat"

# Thin LTO — good balance
[profile.release]
lto = "thin"

# LTO with all crates including dependencies
[profile.release]
lto = "fat"
[profile.release.package."*"]
opt-level = 3
```

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
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy)]
    struct PacketFlags: u8 {
        const KEYFRAME = 0b01;
        const AUDIO = 0b10;
    }
}

struct Packet {
    timestamp: u64,
    size: u64,
    flags: PacketFlags, // 1 byte + 7 padding
}
// Total: 17 bytes (padded to 24 for alignment)
```

### SoA vs AoS

For batch processing, Structure of Arrays often outperforms Array of Structures:

```rust
// AoS — poor cache utilisation if you only read timestamps
struct Packet { timestamp: u64, data: [u8; 1024] }
let packets: Vec<Packet> = ...;
// Iterating timestamps loads entire 1032-byte structs into cache

// SoA — timestamps are contiguous in memory
struct PacketStore {
    timestamps: Vec<u64>,
    data: Vec<[u8; 1024]>,
}
// Iterating timestamps only touches timestamp cache lines
// 16x better cache utilisation for timestamp-only operations
```

### Use `Box<[T]>` Over `Vec<T>` for Fixed-Size Buffers

```rust
// Vec carries capacity + length + pointer (24 bytes on stack)
// Box<[T]> carries pointer + length only (16 bytes) and can't grow
let fixed: Box<[u8]> = vec![0u8; 4096].into_boxed_slice();

// For truly fixed size, use arrays
let fixed: [u8; 4096] = [0; 4096]; // 4096 bytes on stack
```

### Memory Alignment

```rust
// Align for SIMD operations
#[repr(align(32))] // 32-byte alignment for AVX
struct AlignedBuffer {
    data: [u8; 1024],
}

// Or use aligned crate
use aligned::{Aligned, A32};
let buffer: Aligned<A32, [u8; 1024]> = Aligned::new([0u8; 1024]);
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

// Or use tokio::sync::Mutex for async-aware locking
// (But prefer std::sync::Mutex when not holding across await)
```

### Prefer `tokio::task::spawn_blocking` for CPU Work

```rust
// Don't block the async runtime with CPU-intensive work
let result = tokio::task::spawn_blocking(move || {
    expensive_computation(&data)
}).await?;

// For CPU-bound workloads, consider rayon instead of async
let results: Vec<_> = rayon::prelude::*
    .into_par_iter()
    .map(expensive_computation)
    .collect();
```

### Async Runtime Selection

```rust
// Tokio — general purpose, feature-rich
#[tokio::main]
async fn main() { /* ... */ }

// async-std — std-like API
#[async_std::main]
async fn main() { /* ... */ }

// For extreme performance, consider:
// - smol — lightweight, simple
// - glommio — thread-per-core, async I/O for Linux io_uring
```

---

## FFI Performance

### Minimise FFI Boundary Crossings

```rust
// BAD — call FFI function in a loop
for item in &items {
    unsafe { ffi_process(item); } // FFI overhead per item
}

// GOOD — batch process across FFI
let results = unsafe { ffi_process_batch(items.as_ptr(), items.len()) };
// Single FFI call for all items
```

### Use `#[repr(C)]` for FFI Types

```rust
#[repr(C)]
pub struct FfiStruct {
    pub data: *const u8,
    pub len: usize,
    pub callback: Option<extern "C" fn(*const u8, usize)>,
}
// Ensures correct layout for C interop
```

### Avoid Allocations at FFI Boundary

```rust
// BAD — allocate and return string
extern "C" fn get_name() -> *mut c_char {
    let name = CString::new("hello").unwrap();
    name.into_raw() // caller must free with specific function
}

// GOOD — write into caller-provided buffer
extern "C" fn get_name(buf: *mut c_char, len: usize) -> i32 {
    let name = b"hello";
    if len < name.len() { return -1; }
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), buf as *mut u8, name.len()); }
    name.len() as i32
}
```

---

## Error Handling Performance

### Use Result Efficiently

```rust
// Result<T, E> is zero-cost for Ok branch (just the value)
// For small error types, Result is essentially free

// BAD — use Option for errors that need context
fn parse(data: &[u8]) -> Option<Parsed> {
    if data.len() < 4 { return None; } // no context
    Some(Parsed { /* ... */ })
}

// GOOD — use Result with error context
fn parse(data: &[u8]) -> Result<Parsed, ParseError> {
    if data.len() < 4 { return Err(ParseError::TooShort(data.len())); }
    Ok(Parsed { /* ... */ })
}

// For infallible operations, don't use Result at all
fn add(a: i32, b: i32) -> i32 { a + b } // not Result<i32, Infallible>
```

### Use `?` Operator Efficiently

```rust
// The ? operator compiles to efficient branching
fn process(data: &[u8]) -> Result<Output, Error> {
    let parsed = parse(data)?; // branch on Ok/Err
    let validated = validate(parsed)?;
    Ok(transform(validated))
}

// For performance-critical paths, consider:
// - Using unwrap/expect when you know it's safe
// - Using unsafe { unwrap_unchecked() } when you're certain
```

### Avoid Error Allocations

```rust
// BAD — Box<dyn Error> allocates
fn process() -> Result<(), Box<dyn std::error::Error>> { /* ... */ }

// GOOD — Use concrete error types
#[derive(Debug)]
enum MyError { Io(std::io::Error), Parse(ParseError) }
fn process() -> Result<(), MyError> { /* ... */ }

// Or use thiserror for ergonomic error types
use thiserror::Error;
#[derive(Error, Debug)]
enum MyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

---

## I/O & Network Optimisation

### Buffered I/O

```rust
use std::io::{BufReader, BufWriter};

// BAD — unbuffered reads
let mut file = File::open("data.bin")?;
let mut buf = [0u8; 1];
file.read_exact(&mut buf)?; // syscall per byte!

// GOOD — buffered reads
let file = File::open("data.bin")?;
let mut reader = BufReader::with_capacity(8192, file);
let mut buf = [0u8; 1];
reader.read_exact(&mut buf)?; // buffered, fewer syscalls

// For writes, use BufWriter
let file = File::create("output.bin")?;
let mut writer = BufWriter::with_capacity(8192, file);
writer.write_all(&data)?;
writer.flush()?; // ensure all data written
```

### Vectored I/O

```rust
use std::io::IoSlice;

// Write multiple buffers in one syscall
let header = b"header";
let body = b"body";
let footer = b"footer";

let bufs = [
    IoSlice::new(header),
    IoSlice::new(body),
    IoSlice::new(footer),
];
file.write_vectored(&bufs)?; // single syscall
```

### Async I/O with Tokio

```rust
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::TcpStream;

// Async I/O with buffering
let stream = TcpStream::connect("127.0.0.1:8080").await?;
let (reader, writer) = stream.into_split();

let mut reader = BufReader::new(reader);
let mut writer = BufWriter::new(writer);

let mut buf = [0u8; 1024];
reader.read_exact(&mut buf).await?;
writer.write_all(&response).await?;
writer.flush().await?;
```

### Zero-Copy Network I/O

```rust
use tokio::net::{TcpStream, TcpListener};

// Zero-copy splice between sockets (Linux)
#[cfg(target_os = "linux")]
async fn proxy(mut client: TcpStream, mut server: TcpStream) -> std::io::Result<()> {
    use tokio::io::copy;
    let (mut cr, mut cw) = client.split();
    let (mut sr, mut sw) = server.split();
    
    let client_to_server = copy(&mut cr, &mut sw);
    let server_to_client = copy(&mut sr, &mut cw);
    
    tokio::try_join!(client_to_server, server_to_client)?;
    Ok(())
}
```

---

## Streaming & Chunking Patterns

### Process Large Data in Chunks

```rust
// BAD — load entire file into memory
let data = std::fs::read("huge_file.bin")?;
process(&data);

// GOOD — stream processing
let file = File::open("huge_file.bin")?;
let mut reader = BufReader::new(file);
let mut chunk = vec![0u8; 64 * 1024]; // 64KB chunks

loop {
    let bytes_read = reader.read(&mut chunk)?;
    if bytes_read == 0 { break; }
    process(&chunk[..bytes_read]);
}
```

### Lazy Iterators

```rust
// Process lazily without collecting
let processed = data.iter()
    .filter(|x| x.valid)
    .map(|x| x.value * 2)
    .take(100); // stops after 100 items

for value in processed {
    println!("{}", value);
}
// Never allocates intermediate Vec
```

### Pipeline Processing

```rust
// Use channels for pipeline parallelism
let (tx1, rx1) = crossbeam::channel::bounded(100);
let (tx2, rx2) = crossbeam::channel::bounded(100);

// Stage 1: read
std::thread::spawn(move || {
    for item in read_items() {
        tx1.send(item).unwrap();
    }
});

// Stage 2: process
std::thread::spawn(move || {
    for item in rx1 {
        tx2.send(process(item)).unwrap();
    }
});

// Stage 3: write
for item in rx2 {
    write_item(item);
}
```

---

## Const Evaluation & Compile-Time Computation

### Use `const fn` for Compile-Time Computation

```rust
// Compute at compile time
const fn hash(s: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    let mut i = 0;
    while i < s.len() {
        hash ^= s.as_bytes()[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

const TABLE_HASH: u64 = hash("lookup_table");
// No runtime cost
```

### Use `const` for Constants

```rust
// BAD — computed at runtime
fn threshold() -> f64 {
    (2.0_f64).sqrt() * 100.0
}

// GOOD — computed at compile time
const THRESHOLD: f64 = (2.0_f64).sqrt() * 100.0;
```

### Static Initialization

```rust
use std::sync::OnceLock;

// Lazy static initialization
static CONFIG: OnceLock<Config> = OnceLock::new();

fn get_config() -> &'static Config {
    CONFIG.get_or_init(|| Config::load().unwrap())
}

// Or use lazy_static crate
use lazy_static::lazy_static;

lazy_static! {
    static ref CONFIG: Config = Config::load().unwrap();
}
```

---

## Unsafe Optimisation Patterns

### When to Use Unsafe

Unsafe is appropriate when:
1. You need to interface with hardware or FFI
2. You need to bypass bounds checks in hot loops (proven safe by logic)
3. You need to implement safe abstractions (Vec, HashMap, etc.)
4. The safe equivalent is measurably too slow

### Bounds Check Elimination

```rust
// Safe version with bounds checks
fn sum(data: &[u32]) -> u32 {
    data.iter().sum()
}

// Unsafe version without bounds checks (use with caution)
fn sum_unsafe(data: &[u32]) -> u32 {
    let mut sum = 0u32;
    let ptr = data.as_ptr();
    let len = data.len();
    
    for i in 0..len {
        unsafe {
            sum = sum.wrapping_add(*ptr.add(i));
        }
    }
    sum
}

// Better: use get_unchecked in iterator
fn sum_unchecked(data: &[u32]) -> u32 {
    data.iter().map(|x| unsafe { *x.get_unchecked(..).get_unchecked(0) }).sum()
}
// Actually, just use iterators — they eliminate bounds checks automatically
```

### Unsafe Cell for Interior Mutability

```rust
use std::cell::UnsafeCell;

struct UnsafeRingBuffer<T> {
    data: Box<[UnsafeCell<T>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// Single-threaded or synchronized access pattern
impl<T> UnsafeRingBuffer<T> {
    fn push(&self, item: T) {
        let head = self.head.load(Ordering::Relaxed);
        unsafe {
            *self.data[head % self.data.len()].get() = item;
        }
        self.head.store(head + 1, Ordering::Release);
    }
}
```

---

## Thread Pool & Work Stealing

### Use Rayon for Data Parallelism

```rust
use rayon::prelude::*;

// Parallel iteration
let results: Vec<_> = (0..1000)
    .into_par_iter()
    .map(|i| expensive_computation(i))
    .collect();

// Parallel sort
let mut data = vec![/* large vec */];
data.par_sort_unstable();

// Parallel fold
let sum: u64 = (0..1_000_000)
    .into_par_iter()
    .sum();
```

### Custom Thread Pools

```rust
use crossbeam::queue::ArrayQueue;
use std::thread;

struct WorkerPool {
    workers: Vec<thread::JoinHandle<()>>,
    tasks: ArrayQueue<Box<dyn FnOnce() + Send>>,
}

impl WorkerPool {
    fn new(num_workers: usize, queue_size: usize) -> Self {
        let tasks = ArrayQueue::new(queue_size);
        let tasks_clone = tasks.clone(); // Arc internally
        
        let workers = (0..num_workers)
            .map(|_| {
                let tasks = tasks_clone.clone();
                thread::spawn(move || {
                    while let Some(task) = tasks.pop() {
                        task();
                    }
                })
            })
            .collect();
        
        WorkerPool { workers, tasks }
    }
    
    fn submit<F: FnOnce() + Send + 'static>(&self, task: F) {
        self.tasks.push(Box::new(task)).ok();
    }
}
```

### Work Stealing

```rust
// crossbeam-deque provides work-stealing queues
use crossbeam_deque::{Injector, Worker, Stealer};

let injector = Injector::new();
let worker = Worker::new_fifo();
let stealer = worker.stealer();

// Producer pushes to injector
injector.push(task);

// Worker pops from its local queue
worker.push(task);
if let Some(task) = worker.pop() { /* ... */ }

// Other threads can steal
if let Some(task) = stealer.steal() { /* ... */ }
```

---

## GPU & Multimedia Optimisation

### GPU Memory Management

```rust
// Keep GPU memory on GPU — avoid readback
// D3D11 example

// BAD — readback every frame
let staging = device.CreateTexture2D(&staging_desc)?;
device_context.CopyResource(staging, render_target);
let mapped = device_context.Map(staging);
process_cpu(mapped.pData);

// GOOD — process on GPU
let compute_shader = device.CreateComputeShader(/* ... */);
device_context.Dispatch(compute_shader, render_target);
// No CPU readback
```

### Video Pipeline Optimisation

```rust
// Pipeline stages should run in parallel
// Use channels to decouple:

// Capture thread → Encode thread → Buffer thread → Save thread

let (capture_tx, encode_rx) = crossbeam::channel::bounded(4);
let (encode_tx, buffer_rx) = crossbeam::channel::bounded(16);

// Capture thread
thread::spawn(move || {
    loop {
        let frame = capture_frame();
        if capture_tx.send(frame).is_err() { break; }
    }
});

// Encode thread
thread::spawn(move || {
    loop {
        let frame = match encode_rx.recv() {
            Ok(f) => f,
            Err(_) => break,
        };
        let packet = encode(frame);
        if encode_tx.send(packet).is_err() { break; }
    }
});

// Buffer thread
loop {
    let packet = buffer_rx.recv()?;
    buffer.push(packet);
}
```

### Hardware Encoder Selection

```rust
// Prefer hardware encoders for real-time video
// NVENC (NVIDIA) > AMF (AMD) > QSV (Intel) > Software

// Check availability at runtime
fn detect_encoder() -> EncoderType {
    #[cfg(target_os = "windows")]
    {
        if nvenc_available() { return EncoderType::Nvenc; }
        if amf_available() { return EncoderType::Amf; }
        if qsv_available() { return EncoderType::Qsv; }
    }
    EncoderType::Software
}
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
- [ ] **False sharing** — Are atomic counters on the same cache line?
- [ ] **Padding waste** — Group struct fields by size
- [ ] **Unnecessary FFI boundary crossings** — Batch operations
- [ ] **Missing bounds check elimination** — Use iterators or unsafe get_unchecked
- [ ] **Large futures** — Heap-allocate large buffers in async functions
- [ ] **Unbuffered I/O** — Use BufReader/BufWriter
- [ ] **Heap allocations for small collections** — Use SmallVec/ArrayVec
- [ ] **Missing LTO** — Enable for release builds
- [ ] **Wrong atomic ordering** — Use Acquire/Release for data synchronization
- [ ] **Lock-free bugs** — Ensure correct memory ordering, test with ThreadSanitizer

---

## When to Optimise vs When to Ship

Not everything needs to be fast. Apply this decision framework:

1. **Is it in the hot path?** If code runs once at startup or on a button click, don't optimise it. Focus on code that runs per-frame, per-packet, or per-request.
2. **Is it measurably slow?** If you can't show it's slow with a profiler, it's not slow. Intuition about performance is frequently wrong.
3. **Will users notice?** A 2ms improvement in a 500ms operation is irrelevant. A 2ms improvement in a 16ms frame budget is significant.
4. **Does it sacrifice readability?** Clever optimisations that nobody can maintain are a net negative. Prefer clear code with good algorithms over micro-optimised spaghetti.

When in doubt, write clear code first, benchmark it, and only optimise if the numbers demand it.

---

## Performance Optimisation Workflow

Follow this systematic approach:

1. **Measure** — Profile to find the actual bottleneck, not your intuition.
2. **Hypothesise** — Form a theory about why it's slow.
3. **Change** — Make the smallest possible change to test the hypothesis.
4. **Measure again** — Verify the change had the expected effect.
5. **Repeat** — If it worked, look for the next bottleneck. If not, revert.

**Golden rules**:
- Never optimise without profiling data.
- Optimise the hot path, not the cold path.
- Algorithmic improvements beat micro-optimisations.
- Readability matters more than micro-optimisations.
- Correctness first, performance second.

---

## Quick Reference: Performance Cheat Sheet

| Problem | Solution |
|---------|----------|
| Too many allocations | Reuse buffers, use `Bytes`, pre-size collections |
| Lock contention | Reduce critical sections, use lock-free structures, shard |
| Cache misses | SoA layout, prefetch, align to cache lines |
| Bounds checks | Use iterators, `chunks_exact`, or `get_unchecked` |
| Slow string building | `write!` into pre-sized String, `Cow<str>` |
| Large futures | Heap-allocate large buffers |
| Unbounded memory growth | Use bounded channels |
| Slow HashMap | Consider `FxHashMap` or `hashbrown` |
| FFI overhead | Batch operations, minimise crossings |
| Slow I/O | Buffer I/O, use vectored I/O |
| Branch misprediction | Sort data, use branchless techniques |
| SIMD opportunity | Use `wide` crate or portable_simd |
| Compile time too long | Thin LTO, optimise dependencies separately |
| Binary too large | `opt-level = "z"`, strip, LTO |
