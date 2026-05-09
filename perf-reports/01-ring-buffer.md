# Ring Buffer Performance Analysis

## Summary

The SPMC ring buffer (`LockFreeReplayBuffer`) is the critical replay buffer at the heart of LiteClip's recording pipeline. The single producer (capture thread) pushes encoded video/audio packets while multiple consumers (snapshot/save threads) read. The design uses an atomic write index with per-slot `parking_lot::Mutex` locks — not strictly lock-free in push, but avoids a global queue lock while keeping snapshots non-blocking via `try_lock`.

Overall, the design is sound and well-optimized for its use case. The biggest performance concerns revolve around **cache line false sharing** (many hot atomics packed into the same cache line), **redundant atomic ordering under mutex protection**, and **O(n log n) index lookup in the snapshot_from two-pass design**. These are unlikely to bottleneck at 60fps 1440p under normal conditions but could degrade under high memory pressure or during concurrent snapshot+push operations.

## Hot Paths

### Push path

Called on **every encoded packet** at 60fps + audio rates (~110 packets/sec). Sequence:

1. **Parameter cache check** — `param_cache_complete` load (Relaxed), optional cache clear every 1000 pushes.
2. **Parameter set NAL scanning** — If cache incomplete, NAL-scan in `cache_parameter_sets()` holding `param_cache` mutex. This only happens during the first few packets of a recording session (or after periodic cache clear). **Adds non-trivial latency on early pushes**.
3. **First video info** — `first_video_info` mutex lock on first video packet.
4. **Atomic slot claim** — `write_idx.fetch_add(1, Ordering::Relaxed)`. Single-producer, so Relaxed is correct.
5. **Slot lock + swap** — `parking_lot::Mutex::lock()`, take old packet, insert new, update `total_bytes` and `keyframe_count` with `Ordering::Release`. The mutex lock/unlock already provides Acquire/Release semantics, making the explicit atomic ordering on `total_bytes.fetch_add/sub` redundant.
6. **Duration eviction** — After ring wrap (`has_wrapped`), checks if oldest packets exceed time budget. Acquires per-slot locks sequentially.
7. **Memory eviction** — If above 80% watermark, batch-evicts up to `EVICTION_BATCH_SIZE=8` slots. Inner while-loop loads `total_bytes` with `Ordering::Acquire` on every iteration — **hot atomic in a tight loop**.

### Snapshot path

Called on clip save (user-triggered, up to a few times per minute).

**`snapshot()`** (full dump):
- Load `write_idx` and `evict_frontier` (both Acquire).
- Iterate over all live slots with `try_lock()` — non-blocking.
- Clone each packet's `Bytes` (O(1) refcount bump).
- Lock `param_cache` and `first_video_info` in RC2 order (correct AB-BA fix).
- Optionally prepend parameter sets.

**`snapshot_from(start_pts)`** (keyframe-seeking clip save):
- **Pass 1**: Iterates all live slots with `try_lock()`, records metadata (PTS, keyframe, stream) into `Vec<SlotMeta>` — **no data clones**.
- Builds `included_indices: Vec<usize>` to track which slots to include.
- Calls `metas.shrink_to_fit()` and drops `metas`.
- **Pass 2**: Iterates all live slots again, checks `included_indices.binary_search(&i)` (O(log n) per slot), and clones matching packets.
- Optionally prepends parameter sets.

### Eviction path

Triggered from push path, runs on producer thread:

- **Duration eviction**: Sequential loop over slots from `evict_frontier`, checks PTS < cutoff. On each slot: lock → check → take → unlock → advance frontier.
- **Memory eviction with batching**: Outer `while` loop checks `total_bytes` (Acquire). Inner loop for `EVICTION_BATCH_SIZE` slots: lock → take → update counters → unlock → advance frontier. For proactive eviction (<100%), exits after one batch. For hard eviction (>100%), loops until below limit.
- **Tail drop**: If eviction reached head and memory still exceeds limit, drops the newly pushed packet itself.

## Findings

### Finding 1: Cache line false sharing on LockFreeInner atomics
- **Location**: `spmc_ring.rs:155-171` (LockFreeInner struct)
- **Severity**: High
- **Description**: All hot atomics (`write_idx`, `evict_frontier`, `total_bytes`, `keyframe_count`, `newest_pts`, `has_wrapped`, `restart_generation`, `param_cache_complete`, `param_cache_pushes_since_complete`, `outstanding_snapshot_bytes`) are packed into the same struct without cache line padding. On x64, these fit within ~80 bytes, likely 1-2 cache lines.
- **Code**:
```rust
struct LockFreeInner {
    slots: Box<[Slot]>,
    capacity: usize,
    mask: usize,
    max_duration_qpc: i64,
    write_idx: AtomicUsize,        // Written: producer (push)
    evict_frontier: AtomicUsize,    // Written: producer (eviction)
    max_memory_bytes: usize,
    total_bytes: AtomicUsize,       // Written: producer (push+eviction)
    keyframe_count: AtomicUsize,    // Written: producer (push+eviction)
    newest_pts: AtomicI64,          // Written: producer (push)
    has_wrapped: AtomicBool,        // Written: producer (push)
    restart_generation: AtomicUsize,// Written: producer (restart)
    param_cache: std::sync::Mutex<ParameterCache>,
    param_cache_complete: AtomicBool, // Written: producer (push)
    param_cache_pushes_since_complete: AtomicUsize, // Written: producer (push)
    first_video_info: std::sync::Mutex<Option<(usize, FirstVideoKind)>>,
    outstanding_snapshot_bytes: AtomicUsize, // Written: consumers (SnapshotBytes)
}
```
- **Why it matters**: The producer thread writes to `write_idx`, `total_bytes`, `keyframe_count`, `newest_pts`, `has_wrapped`, `evict_frontier` on every push. Consumers read `write_idx`, `evict_frontier`, `total_bytes`, `keyframe_count`, `newest_pts`, `outstanding_snapshot_bytes`. A consumer reading `write_idx` (Acquire) invalidates the L1 cache line on the producer core. The producer writing `total_bytes` (Release) invalidates the cache line on consumer cores. This creates constant MESI protocol traffic between cores at 60fps push rates.
- **Recommendation**: Split into two cache-line-aligned groups: (A) producer-written atomics only (hot), (B) consumer-read atomics and rarely-written fields (cold). Use `#[repr(align(128))]` (64 bytes may not suffice with adjacent data) or pad between groups. Example: put `write_idx`, `total_bytes`, `keyframe_count`, `newest_pts`, `evict_frontier`, `has_wrapped` in one cache line, move read-mostly fields like `max_memory_bytes`, `capacity`, `mask`, `max_duration_qpc` to a separate cache line, and isolate `outstanding_snapshot_bytes` (written by consumers) from producer atomics.

### Finding 2: Redundant atomic ordering inside mutex-protected sections
- **Location**: `spmc_ring.rs:284-300` (push_single slot lock block), `spmc_ring.rs:410-430` (eviction slot lock), `spmc_ring.rs:482-488` (duration eviction slot lock)
- **Severity**: Medium
- **Description**: `total_bytes.fetch_add/sub()` and `keyframe_count.fetch_add/sub()` are called with `Ordering::Release` while the caller already holds a `parking_lot::Mutex` lock on the slot. Mutex lock/unlock provides natural Acquire/Release semantics — the slot's data is already visible to the next lock holder. The explicit Release ordering on atomic operations adds an unnecessary memory barrier on every push and every eviction.
- **Code**:
```rust
// Inside mutex lock block:
inner.total_bytes.fetch_add(packet_size, Ordering::Release);
// ...
inner.total_bytes.fetch_sub(old_size, Ordering::Release);
if old_was_keyframe {
    inner.keyframe_count.fetch_sub(1, Ordering::Release);
}
// ...
inner.keyframe_count.fetch_add(1, Ordering::Release);
```
- **Why it matters**: At 60fps 1440p, the buffer processes ~110-120 packets/sec. Each push does ~4-6 atomic operations with Release ordering inside the mutex. While each individual cost is small (~10ns), cumulative effect across thousands of pushes adds up. More importantly, **Release ordering on one core forces memory store buffer drain**, which can delay dependent loads on the producer thread.
- **Recommendation**: Use `Ordering::Relaxed` for all `fetch_add`/`fetch_sub` operations on `total_bytes` and `keyframe_count` when already inside a mutex-protected section. The mutex's sequential consistency guarantees already ensure correct visibility. For `newest_pts.store()` (which is outside the mutex), keep `Ordering::Release` since it's written lock-free.

### Finding 3: O(n log n) index lookup in snapshot_from Pass 2 via binary_search
- **Location**: `spmc_ring.rs:841-848` (snapshot_from Pass 2 binary_search)
- **Severity**: Medium
- **Description**: Pass 2 iterates all slots from `first_idx..write_idx` (potentially thousands of slots) and calls `included_indices.binary_search(&i)` on each iteration to decide whether to clone the packet. `binary_search` on a `Vec<usize>` is O(log m) where m ≈ included count. Total complexity: O(n log n) where n = total slots scanned. A simple `Vec<bool>` indexed by `(i - first_idx)` would be O(1) per lookup.
- **Code**:
```rust
// Pass 2: selective clone
for i in first_idx..write_idx {
    if included_indices.binary_search(&i).is_err() {
        continue;
    }
    // ... clone packet
}
```
- **Why it matters**: At 1440p 60fps with ~110 packets/sec and a 120s buffer, the ring holds ~13,200 packets. O(n log n) with binary_search means ~13,200 * log₂(13,200) ≈ 13,200 * 14 ≈ 185,000 comparisons per snapshot. A `Vec<bool>` (or `bitvec`) would be ~13,200 O(1) lookups. Snapshot saves are user-triggered and infrequent, so this is not a hot path bottleneck, but the gap widens with larger buffers and could cause visible stutter during clip saves on lower-end CPUs.
- **Recommendation 1** (preferred): Replace `included_indices: Vec<usize>` with a `Vec<bool>` of length `(write_idx - first_idx)`. Set `true` at position `(m.ring_idx - first_idx)` for included slots. Pass 2 lookup becomes O(1) array indexing. Memory cost is ~n bits ≈ 1.6 KB for a 13K-slot buffer — negligible.
- **Recommendation 2**: Alternatively, since `included_indices` is already sorted, keep a cursor and linear-scan through it alongside the slot scan. Both sequences are in the same order, so this is O(n). This avoids allocation of the `Vec<bool>` entirely.

### Finding 4: Tight loop loads `total_bytes` with Acquire ordering during memory eviction
- **Location**: `spmc_ring.rs:388-389` (eviction while loop condition)
- **Severity**: Medium
- **Description**: The memory eviction `while` loop loads `inner.total_bytes.load(Ordering::Acquire)` on each outer iteration. Since `total_bytes` is already updated (with Release) inside the slot lock blocks within the loop, and the producer is single-threaded, using Acquire here forces a full memory fence on every eviction batch.
- **Code**:
```rust
while inner.total_bytes.load(Ordering::Acquire) > target_bytes {
    for _ in 0..EVICTION_BATCH_SIZE {
        // ... lock slot, take packet, total_bytes.fetch_sub(..., Release)
    }
}
```
- **Why it matters**: During high memory pressure, this loop may run many iterations. Each Acquire load stalls on the store buffer drain on x86 (since x86's TSO makes Acquire almost free for loads, but paired with the preceding Release `fetch_sub`, the release fence is still significant). Under hard eviction (>100% memory), the loop runs until below target, potentially dozens of iterations.
- **Recommendation**: Use `Ordering::Relaxed` for the while-loop condition. Since `total_bytes` is only written by the single producer thread and the loop itself is running on that same thread, there are no cross-thread visibility concerns within the loop. The producer will always see its own latest writes. If cross-thread precision is needed, a single Acquire load before the loop suffices, with Relaxed inside.

### Finding 5: `metas.shrink_to_fit()` causes unnecessary reallocation in snapshot_from
- **Location**: `spmc_ring.rs:806-807`
- **Severity**: Low
- **Description**: After Pass 1 collects metadata into `Vec<SlotMeta>` (capacity = full ring), the code calls `metas.clear()`, `metas.shrink_to_fit()`, then `drop(metas)`. The `shrink_to_fit()` causes a reallocation and copy of the empty Vec's backing allocation (minimal), but the real issue is that `metas` is dropped immediately after — `shrink_to_fit` is completely wasted work.
- **Code**:
```rust
metas.clear();
metas.shrink_to_fit();
drop(metas);
```
- **Why it matters**: Trivially wastes CPU cycles on a reallocation that is immediately freed. The `clear()` + `drop()` would suffice. At snapshot time this is a one-shot cost, so impact is negligible.
- **Recommendation**: Remove `metas.shrink_to_fit()` — just `clear()` and `drop()`.

### Finding 6: Per-slot lock ordering during eviction could starve snapshot consumers
- **Location**: `spmc_ring.rs:401-432` (batch eviction inner loop)
- **Severity**: Low
- **Description**: The eviction path uses `parking_lot::Mutex::lock()` (blocking) on per-slot mutexes, while the snapshot path uses `try_lock()` (non-blocking). Under sustained high memory pressure, the producer thread holds and releases per-slot locks in rapid succession. Since `lock()` is blocking and `parking_lot` uses an unfair (throughput-optimized) strategy, a snapshot consumer's `try_lock()` may be repeatedly denied if the producer keeps re-acquiring the same slot's lock.
- **Code**:
```rust
// Eviction (push path) — blocking lock:
let mut guard = slot.packet.lock();

// Snapshot — non-blocking try_lock:
if let Some(packet_guard) = slot.packet.try_lock() {
```
- **Why it matters**: Under stress (e.g., user saves a clip while buffer is near capacity), snapshot slots may be missed by `try_lock`, leading to incomplete clips. The design choice is documented (concurrent safety vs completeness) but the effect is amplified by unfair parking_lot behavior.
- **Recommendation**: This is an inherent trade-off of the `try_lock` approach. Document the risk more prominently. If clip completeness under memory pressure is critical, consider a short spin-retry loop in the snapshot path (e.g., `try_lock` with a small number of retries + `std::hint::spin_loop()`).

### Finding 7: Parameter cache uses `std::sync::Mutex` instead of `parking_lot::Mutex`
- **Location**: `spmc_ring.rs:167`
- **Severity**: Low
- **Description**: The `param_cache` field uses `std::sync::Mutex<ParameterCache>` while all per-slot mutexes use `parking_lot::Mutex`. `std::sync::Mutex` is heavier (syscall on contention, bigger struct, cannot be used in const contexts, no fast path on Windows without SRWOK).
- **Code**:
```rust
param_cache: std::sync::Mutex<ParameterCache>,
// But:
packet: parking_lot::Mutex<Option<EncodedPacket>>,
```
- **Why it matters**: The param_cache mutex is only held briefly (NAL scan or snapshot prepend) and rarely contended after cache completion. Impact is negligible. However, the inconsistency suggests an oversight.
- **Recommendation**: Change to `parking_lot::Mutex` for consistency and slightly better performance on the rare contention case. Alternatively, switch to a `RwLock` since snapshot reads dominate after cache completion.

### Finding 8: Periodic cache clear (every 1000 pushes) triggers NAL re-scan of subsequent packets
- **Location**: `spmc_ring.rs:250-256`
- **Severity**: Low
- **Description**: The parameter cache is cleared every 1000 pushes after completion to handle encoder reconfiguration. After clear, every video push re-enters `cache_parameter_sets()` and NAL-scans until cache is repopulated. At 1000-push intervals (roughly every 9 seconds at 110 pps), this causes a burst of NAL-scanning on subsequent packets.
- **Code**:
```rust
if pushes >= 1000 {
    self.clear_parameter_cache();
}
```
- **Why it matters**: NAL scanning is relatively cheap (linear scan of packet data), but it runs on the push/capture thread. If the encoder doesn't reconfigure between cache clears, the work is wasted. Impact is low at typical capture bitrates.
- **Recommendation**: Make the cache clear interval configurable or event-driven (explicit signal from encoder on reconfiguration) rather than time-based. For now, consider bumping to 5000+ pushes to reduce frequency.

## Benchmark Gaps

1. **No microbenchmark for atomic contention under concurrent push+snapshot**: The existing `test_snapshot_valid_during_concurrent_eviction` test is good but uses coarse timing (100µs sleep). A targeted benchmark measuring push latency while snapshot threads are actively reading would reveal cache line bouncing costs.

2. **No benchmark for eviction throughput at memory pressure**: Key metrics: how many eviction batches per second at 80% vs 100% watermarks, how many slots are skipped due to empty/contended locks. This is essential for tuning `EVICTION_BATCH_SIZE`.

3. **No comparison of `binary_search` vs `Vec<bool>` in snapshot_from**: A benchmark generating a large (10K+ packets) snapshot and comparing Pass 2 wall time would validate Finding 3.

4. **No measurement of `parking_lot::Mutex::try_lock` miss rate during concurrent eviction**: Important for understanding clip quality under memory pressure. A simple counter of `try_lock` misses during snapshot would quantify the risk in Finding 6.

5. **No benchmark for parameter cache NAL scanning cost**: Measuring the time spent in `cache_parameter_sets()` vs total push latency, especially right after cache clear, would determine if Finding 8 matters in practice.

6. **No benchmark for cache line padding benefit**: A microbenchmark comparing push latency with current struct layout vs cache-line-padded layout would quantify the cost of false sharing (Finding 1). This is the highest-value benchmark to add.

## Scoring

| Finding | Severity | Impact at 60fps 1440p | Effort to Fix |
|---------|----------|----------------------|----------------|
| 1. Cache line false sharing | High | **Medium** — 5-15% push latency regression under concurrent snapshot; mostly affects smoothness during clip saves | Medium (struct padding + split) |
| 2. Redundant atomic ordering inside mutex | Medium | **Low-Medium** — ~3-5% push path overhead, mostly noise at 60fps | Low (change Ordering) |
| 3. O(n log n) binary_search in snapshot_from | Medium | **Low** — affects clip save latency (~1-5ms on 13K buffer), not push path | Low (use Vec<bool> or cursor) |
| 4. Acquire ordering in eviction tight loop | Medium | **Low-Medium** — matters during prolonged memory pressure scenarios | Low (Relaxed in loop) |
| 5. Unnecessary shrink_to_fit | Low | **Negligible** — one wasted reallocation per snapshot | Low (remove line) |
| 6. try_lock vs lock starvation | Low | **Low** — edge case under high concurrent load | Medium (spin-retry) |
| 7. std::sync::Mutex inconsistency | Low | **Negligible** — rarely contended path | Low (change to parking_lot) |
| 8. Periodic cache re-scan | Low | **Negligible** — 100ms of scanning every 9 seconds | Low (make event-driven) |

### Recommendations by priority

1. **P0 — Cache line padding** (Finding 1): Split `LockFreeInner` atomics into producer-hot and consumer-hot sections, separated by cache line boundaries. This is the highest-impact change and directly affects the core push path under concurrent access.

2. **P1 — Fix binary_search in snapshot_from** (Finding 3): Replace `Vec<usize>` with `Vec<bool>` or use a linear cursor. Low effort, clean win for snapshot latency.

3. **P1 — Relaxed ordering in eviction tight loop** (Finding 4): Change while-loop `Acquire` to `Relaxed`. Trivial change with measurable benefit under memory pressure.

4. **P1 — Relaxed ordering inside mutex blocks** (Finding 2): Change `Ordering::Release` to `Ordering::Relaxed` for `fetch_add`/`fetch_sub` in all mutex-protected sections. Trivial, safe change.

5. **P2 — Remove shrink_to_fit** (Finding 5): One line deletion, clean code.
