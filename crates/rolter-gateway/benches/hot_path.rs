//! Hot-path benches with allocation counting (#1054).
//!
//! `admission.rs` times the breaker and cooldown registries on their own. This
//! bench covers what a request actually pays on the way to a target, and counts
//! the allocations alongside the timings, because wall-clock numbers on a
//! shared runner are too noisy to defend a claim like "the hot path is
//! allocation-light" and an allocation count is not:
//!
//! - `admission` — breaker `allows`, cooldown `is_parked` and provider health
//!   on a route with every target healthy, the steady state
//! - `eligible_pick` — one balancer pick the way `TargetPool::pick` makes it: the
//!   eligibility closure asks the three registries about each target, with a
//!   parked and an unhealthy target in the pool
//! - `snapshot_read` — the config read every request starts with: an
//!   `ArcSwap` load and the route lookup
//! - `key_pool` — a multi-key provider's key pick asking the cooldown registry
//!   which of its 8 keys are parked: once per key (before #3249) and as one
//!   mask read
//!
//! Before the criterion run, `main` prints an `allocs/op` table. The counting
//! allocator is installed for this bench binary only and counts the measuring
//! thread's allocations while armed, as `tests/hot_path_allocations.rs` does.
//!
//! Run with `cargo bench -p rolter-gateway --bench hot_path` (`just bench-hot-path`).

use criterion::{criterion_group, BenchmarkId, Criterion};
use rolter_balancer::{build, LoadBalancer, RouteContext};
use rolter_core::{BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, Target};
use rolter_gateway::breaker::Breaker;
use rolter_gateway::cooldowns::Cooldowns;
use rolter_gateway::health::Health;
use rolter_gateway::target_mask::TargetMask;
use rolter_gateway::AppState;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

struct CountingAllocator;

fn record_allocation() {
    // try_with: during thread teardown the slot may be gone, and an allocation
    // then is not part of any measurement anyway
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        }
    });
}

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
        record_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Allocations per call of `op`, averaged over `runs` calls on this thread.
fn allocs_per_op(runs: usize, mut op: impl FnMut()) -> f64 {
    // one unmeasured call, so lazy one-time state is not charged to the steady state
    op();
    ALLOCATIONS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    for _ in 0..runs {
        op();
    }
    ARMED.with(|a| a.set(false));
    ALLOCATIONS.with(|n| n.get()) as f64 / runs as f64
}

/// A realistic public model name: long enough that a per-call `String` copy is
/// a real allocation rather than something an allocator can hide.
const MODEL: &str = "anthropic/claude-sonnet-4-5-20250929";

/// Pool sizes the pick benches run at: a small route, a self-hosted replica
/// set, and one past the 64 targets a `u64` mask can hold.
const POOLS: [usize; 4] = [4, 24, 64, 128];

const PICK_STRATEGIES: [(&str, BalancingStrategy); 4] = [
    ("round_robin", BalancingStrategy::RoundRobin),
    ("power_of_two", BalancingStrategy::PowerOfTwo),
    ("cache_aware", BalancingStrategy::CacheAware),
    ("pipeline", BalancingStrategy::Pipeline),
];

/// The three registries the eligibility closure consults, populated like a
/// route with one target parked on a cooldown and another marked unhealthy.
struct Fleet {
    cooldowns: Cooldowns,
    health: Health,
    breaker: Breaker,
    providers: Vec<String>,
}

impl Fleet {
    fn new(n: usize) -> Self {
        let providers: Vec<String> = (0..n).map(|i| format!("provider-{i}")).collect();
        let cooldowns = Cooldowns::new();
        let health = Health::new();
        let breaker = Breaker::new(true, 5, 30);
        cooldowns.park(MODEL, 0, 3600);
        health.set(&providers[1], false);
        // a live closed entry on a few targets, so lookups hit the map
        for idx in 2..n.min(6) {
            breaker.on_failure(MODEL, idx);
        }
        Self {
            cooldowns,
            health,
            breaker,
            providers,
        }
    }

    /// The eligibility check as it was made per target, three lock
    /// acquisitions each: the shape `pick_untried` had before #1942.
    fn eligible(&self, idx: usize) -> bool {
        !self.cooldowns.is_parked(MODEL, idx)
            && self.health.is_healthy(&self.providers[idx])
            && self.breaker.allows(MODEL, idx)
    }

    /// One read of each registry for the whole route: three lock acquisitions
    /// per attempt, whatever the pool size.
    fn snapshot(&self) -> Snapshot {
        let n = self.providers.len();
        Snapshot {
            parked: self.cooldowns.parked_mask(MODEL, n),
            unhealthy: self
                .health
                .unhealthy_mask(self.providers.iter().map(String::as_str)),
            open: self.breaker.open_mask(MODEL, n),
        }
    }
}

struct Snapshot {
    parked: TargetMask,
    unhealthy: TargetMask,
    open: TargetMask,
}

impl Snapshot {
    fn eligible(&self, idx: usize) -> bool {
        !(self.parked.contains(idx) || self.unhealthy.contains(idx) || self.open.contains(idx))
    }
}

/// In-flight loads where the two dead targets read as the emptiest.
fn loads(n: usize) -> Vec<u64> {
    (0..n)
        .map(|i| if i < 2 { 0 } else { 3 + (i as u64 * 7) % 11 })
        .collect()
}

fn context() -> RouteContext<'static> {
    RouteContext::new()
        .with_session_key(Some("user-42-session-abcdef0123456789"))
        .with_prompt(Some(
            "summarize the following document for me in three concise bullet points: lorem \
             ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor",
        ))
}

fn balancer(strategy: BalancingStrategy, n: usize, ctx: &RouteContext) -> Box<dyn LoadBalancer> {
    let weights: Vec<u32> = (0..n).map(|i| 1 + (i as u32 % 4)).collect();
    let lb = build(strategy, &weights);
    // let strategies that learn from traffic hold something to match against
    for i in 0..n {
        lb.observe(i, ctx);
    }
    lb
}

fn route_config(routes: usize) -> GatewayConfig {
    let providers: Vec<ProviderConfig> = (0..8)
        .map(|i| ProviderConfig {
            name: format!("provider-{i}"),
            slug: Some(format!("provider-{i}")),
            api_base: format!("https://provider-{i}.example.test/v1"),
            api_key_env: Some(format!("PROVIDER_{i}_KEY")),
            ..Default::default()
        })
        .collect();
    let routes = (0..routes)
        .map(|r| ModelRoute {
            model: format!("model-{r}"),
            strategy: BalancingStrategy::RoundRobin,
            targets: (0..3)
                .map(|t| Target {
                    provider: format!("provider-{}", (r + t) % 8),
                    model: Some(format!("upstream-model-{r}")),
                    ..Default::default()
                })
                .collect(),
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            tenancy: None,
        })
        .collect();
    GatewayConfig {
        providers,
        routes,
        ..Default::default()
    }
}

/// A pick the way `pick_untried` made it before #1942: the balancer asks the
/// three registries about each target it weighs, then the pick is judged again.
fn pick_per_target(
    lb: &dyn LoadBalancer,
    ctx: &RouteContext,
    loads: &[u64],
    fleet: &Fleet,
) -> bool {
    let picked = lb.pick_eligible(ctx, loads, &|i| fleet.eligible(i));
    picked.is_some_and(|i| fleet.eligible(i))
}

/// A pick the way `TargetPool::pick` makes it now: the registries are read once, and
/// only if the balancer asks, then the pick is judged on its own.
fn pick_snapshot(lb: &dyn LoadBalancer, ctx: &RouteContext, loads: &[u64], fleet: &Fleet) -> bool {
    let seen = std::cell::OnceCell::new();
    let picked = lb.pick_eligible(ctx, loads, &|i| {
        seen.get_or_init(|| fleet.snapshot()).eligible(i)
    });
    picked.is_some_and(|i| fleet.eligible(i))
}

fn admission(c: &mut Criterion) {
    let mut group = c.benchmark_group("admission");
    let fleet = Fleet::new(8);
    group.bench_function("three_registries_8_targets", |b| {
        b.iter(|| {
            for idx in 0..8 {
                black_box(fleet.eligible(black_box(idx)));
            }
        });
    });
    group.finish();
}

fn eligible_pick(c: &mut Criterion) {
    let ctx = context();
    for (label, strategy) in PICK_STRATEGIES {
        let mut group = c.benchmark_group(format!("eligible_pick/{label}"));
        for n in POOLS {
            let fleet = Fleet::new(n);
            let loads = loads(n);
            let lb = balancer(strategy, n, &ctx);
            group.bench_with_input(BenchmarkId::new("per_target_locks", n), &n, |b, _| {
                b.iter(|| black_box(pick_per_target(&*lb, &ctx, &loads, &fleet)));
            });
            group.bench_with_input(BenchmarkId::new("snapshot", n), &n, |b, _| {
                b.iter(|| black_box(pick_snapshot(&*lb, &ctx, &loads, &fleet)));
            });
        }
        group.finish();
    }
}

fn snapshot_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot_read");
    for routes in [10usize, 1000] {
        let state = AppState::with_logging(&route_config(routes), None);
        let model = format!("model-{}", routes / 2);
        group.bench_with_input(
            BenchmarkId::new("load_and_route", routes),
            &routes,
            |b, _| {
                b.iter(|| {
                    let snap = state.snapshot.load();
                    black_box(snap.routes.get("", black_box(model.as_str())).is_some())
                });
            },
        );
    }
    group.finish();
}

/// The key-pool namespace a multi-key provider's keys are parked under.
const KEY_NS: &str = "key::provider-0";
const KEYS: usize = 8;

fn key_pool(c: &mut Criterion) {
    let mut group = c.benchmark_group("key_pool");
    let cooldowns = Cooldowns::new();
    cooldowns.park(KEY_NS, 1, 3600);
    group.bench_function("per_key_locks_8_keys", |b| {
        b.iter(|| {
            for i in 0..KEYS {
                black_box(cooldowns.is_parked(black_box(KEY_NS), i));
            }
        });
    });
    group.bench_function("mask_8_keys", |b| {
        b.iter(|| {
            let parked = cooldowns.parked_mask(black_box(KEY_NS), KEYS);
            for i in 0..KEYS {
                black_box(parked.contains(i));
            }
        });
    });
    group.finish();
}

criterion_group!(benches, admission, eligible_pick, snapshot_read, key_pool);

/// Print allocations per operation for the paths above, before the timings.
fn report_allocations() {
    const RUNS: usize = 2_000;
    let ctx = context();
    eprintln!("allocs/op (counting allocator, measuring thread, steady state)");

    let fleet = Fleet::new(8);
    let n = allocs_per_op(RUNS, || {
        for idx in 0..8 {
            black_box(fleet.eligible(black_box(idx)));
        }
    });
    eprintln!("  admission/three_registries_8_targets        {n:>8.2}");

    for (label, strategy) in PICK_STRATEGIES {
        for pool in POOLS {
            let fleet = Fleet::new(pool);
            let loads = loads(pool);
            let lb = balancer(strategy, pool, &ctx);
            let n = allocs_per_op(RUNS, || {
                black_box(pick_per_target(&*lb, &ctx, &loads, &fleet));
            });
            eprintln!("  eligible_pick/{label}/per_target_locks/{pool:<4} {n:>8.2}");
            let n = allocs_per_op(RUNS, || {
                black_box(pick_snapshot(&*lb, &ctx, &loads, &fleet));
            });
            eprintln!("  eligible_pick/{label}/snapshot/{pool:<4}        {n:>8.2}");
        }
    }

    let state = AppState::with_logging(&route_config(1000), None);
    let model = "model-500";
    let n = allocs_per_op(RUNS, || {
        let snap = state.snapshot.load();
        black_box(snap.routes.get("", black_box(model)).is_some());
    });
    eprintln!("  snapshot_read/load_and_route/1000           {n:>8.2}");

    // an org key resolving a name two orgs share (#1857): the two-level lookup
    // borrows both keys and allocates nothing
    let mut shared = route_config(1000);
    for org in ["org-a", "org-b"] {
        let mut route = shared.routes[500].clone();
        route.tenancy = Some(rolter_core::Tenancy {
            org_id: org.to_string(),
            project_id: None,
        });
        shared.routes.push(route);
    }
    let state = AppState::with_logging(&shared, None);
    let n = allocs_per_op(RUNS, || {
        let snap = state.snapshot.load();
        black_box(
            snap.routes
                .get(black_box("org-b"), black_box(model))
                .is_some(),
        );
    });
    eprintln!("  snapshot_read/org_key_shared_name/1000      {n:>8.2}");

    let cooldowns = Cooldowns::new();
    cooldowns.park(KEY_NS, 1, 3600);
    let n = allocs_per_op(RUNS, || {
        let parked = cooldowns.parked_mask(black_box(KEY_NS), KEYS);
        for i in 0..KEYS {
            black_box(parked.contains(i));
        }
    });
    eprintln!("  key_pool/mask_8_keys                        {n:>8.2}");
    eprintln!();
}

fn main() {
    report_allocations();
    benches();
    Criterion::default().configure_from_args().final_summary();
}
