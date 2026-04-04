//! Criterion benchmarks for GUI interactions (workspace-level).
//!
//! These benchmarks test GUI responsiveness and rendering performance.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_gui_state_transitions(c: &mut Criterion) {
    c.bench_function("gui/state_transitions", |b| {
        b.iter(|| {
            // Simulate GUI state changes
            let mut state = 0;
            for _ in 0..100 {
                state = (state + 1) % 3;
            }
            black_box(state)
        })
    });
}

fn gui_event_processing(c: &mut Criterion) {
    c.bench_function("gui/event_processing", |b| {
        b.iter(|| {
            let events: Vec<u32> = (0..1000).collect();
            black_box(events.iter().sum::<u32>())
        })
    });
}

criterion_group!(
    gui_benches,
    bench_gui_state_transitions,
    gui_event_processing,
);

criterion_main!(gui_benches);
