//! Allocation counting for the data plane's hottest admission checks (#1054).
//!
//! AGENTS.md requires the hot path stay allocation-light, and
//! `benches/admission.rs` explains that `Breaker::allows` and
//! `Cooldowns::is_parked` once built a `(String, usize)` key — an allocation
//! per candidate target, on every upstream attempt, including the healthy
//! steady state where the answer is an immediate "admitted". That regression
//! was fixed, but nothing would notice it coming back: criterion timings are
//! too noisy on shared runners to gate on, so the benches are compiled in CI
//! and never run.
//!
//! This test closes that gap by counting allocations directly rather than
//! inferring them from timings. It asserts **zero** allocations on the steady
//! state paths rather than a numeric budget: an exact count would be a
//! portability trap across allocators and toolchains, whereas "this path does
//! not allocate at all" is both stable and the property actually wanted.
//!
//! # Why the counter is per-thread
//!
//! A process-wide counter is wrong here and measurably so: cargo runs the
//! tests in this binary on parallel threads, so a sibling test's ordinary
//! allocations land inside another test's measurement window and fail it for
//! something it never did. An earlier draft of this file had exactly that bug
//! — injecting an allocation into `Breaker::allows` failed the two `Cooldowns`
//! tests as well, which touch none of the changed code.
//!
//! So arming and counting are both thread-local: only allocations made by the
//! measuring thread, while that thread is inside `counting`, are counted. The
//! thread-locals are `Cell`s with `const` initialisers and no destructor, so
//! reading them from inside the allocator cannot itself allocate or recurse,
//! and `try_with` keeps access safe during thread teardown.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use rolter_gateway::breaker::Breaker;
use rolter_gateway::cooldowns::Cooldowns;

thread_local! {
    /// Allocations this thread made while armed.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    /// Whether this thread is inside a measurement.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

fn record_allocation() {
    // try_with: during thread teardown the slot may be gone, and an allocation
    // then is not part of any measurement anyway
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        }
    });
}

struct CountingAllocator;

// SAFETY: every method forwards to `System` unchanged; the counters are only
// observed, never allowed to affect the pointer returned to the caller.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // a realloc that moves is an allocation for our purposes: it is the
        // growth of a per-request buffer, which is what we are watching for
        record_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Run `body` with allocation counting armed on this thread, and return how
/// many allocations this thread made during it.
fn counting(body: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    body();
    ARMED.with(|a| a.set(false));
    ALLOCATIONS.with(|n| n.get())
}

/// Matches `benches/admission.rs`: long enough that a per-call `String` copy
/// would be a real allocation rather than something an allocator hides.
const MODEL: &str = "anthropic/claude-sonnet-4-5-20250929";
const TARGETS: usize = 8;

#[test]
fn the_probe_itself_observes_allocations() {
    // guards against the counter silently measuring nothing — without this, a
    // broken probe would make every assertion below pass vacuously
    let seen = counting(|| {
        let v: Vec<u8> = vec![1];
        std::hint::black_box(&v);
    });
    assert!(seen > 0, "the counting allocator observed no allocation");
}

#[test]
fn counting_is_disarmed_outside_a_measurement() {
    counting(|| {});
    let before = ALLOCATIONS.with(|n| n.get());
    let v: Vec<u8> = vec![1];
    std::hint::black_box(&v);
    assert_eq!(
        ALLOCATIONS.with(|n| n.get()),
        before,
        "allocations outside a measurement were still counted"
    );
}

#[test]
fn another_thread_allocating_does_not_pollute_a_measurement() {
    // the property the per-thread counter exists for: this is what a parallel
    // sibling test does to a process-wide counter
    let (tx, rx) = std::sync::mpsc::channel();
    let noisy = std::thread::spawn(move || {
        while rx.try_recv().is_err() {
            let v: Vec<u8> = Vec::with_capacity(64);
            std::hint::black_box(&v);
        }
    });

    let seen = counting(|| {
        for _ in 0..10_000 {
            std::hint::black_box(1_usize);
        }
    });

    let _ = tx.send(());
    noisy.join().expect("noisy thread joins");

    assert_eq!(
        seen, 0,
        "another thread's allocations were counted ({seen})"
    );
}

#[test]
fn breaker_admission_on_a_healthy_fleet_does_not_allocate() {
    let breaker = Breaker::new(true, 5, 30);
    // warm any lazily-built internals so we measure the steady state, not
    // first-call setup
    for idx in 0..TARGETS {
        std::hint::black_box(breaker.allows(MODEL, idx));
    }

    let seen = counting(|| {
        for idx in 0..TARGETS {
            std::hint::black_box(breaker.allows(std::hint::black_box(MODEL), idx));
        }
    });

    assert_eq!(
        seen, 0,
        "Breaker::allows allocated {seen} time(s) over {TARGETS} healthy targets; \
         this is the per-attempt hot path and must stay allocation-free"
    );
}

#[test]
fn breaker_admission_with_live_entries_does_not_allocate() {
    let breaker = Breaker::new(true, 5, 30);
    for idx in 0..TARGETS {
        breaker.on_failure(MODEL, idx);
    }

    let seen = counting(|| {
        for idx in 0..TARGETS {
            std::hint::black_box(breaker.allows(std::hint::black_box(MODEL), idx));
        }
    });

    assert_eq!(
        seen, 0,
        "Breaker::allows allocated {seen} time(s) against live entries"
    );
}

#[test]
fn cooldown_lookup_on_a_healthy_fleet_does_not_allocate() {
    let cooldowns = Cooldowns::new();
    for idx in 0..TARGETS {
        std::hint::black_box(cooldowns.is_parked(MODEL, idx));
    }

    let seen = counting(|| {
        for idx in 0..TARGETS {
            std::hint::black_box(cooldowns.is_parked(std::hint::black_box(MODEL), idx));
        }
    });

    assert_eq!(
        seen, 0,
        "Cooldowns::is_parked allocated {seen} time(s) over {TARGETS} healthy targets"
    );
}

#[test]
fn cooldown_lookup_with_a_parked_target_does_not_allocate() {
    let cooldowns = Cooldowns::new();
    cooldowns.park(MODEL, 0, 3600);

    let seen = counting(|| {
        for idx in 0..TARGETS {
            std::hint::black_box(cooldowns.is_parked(std::hint::black_box(MODEL), idx));
        }
    });

    assert_eq!(
        seen, 0,
        "Cooldowns::is_parked allocated {seen} time(s) with a parked target"
    );
}
