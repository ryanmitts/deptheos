//! Opt-in stage timing for benchmarks: GPU time per compute pass from
//! timestamp queries, and CPU time per named scope.
//!
//! Off by default, when every hook is one relaxed atomic load. A bench calls
//! [`enable`], runs, then [`take_report`], which resolves every query in one
//! extra submission so no submit site knows about the profiler.
//! Process-global because the dispatch helpers have no context: one device
//! per process.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wgpu_profiler::{GpuProfiler, GpuProfilerQuery, GpuProfilerSettings, GpuTimerQueryResult};

use crate::context::GpuContext;

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<State>> = Mutex::new(None);

struct State {
    /// `None` when the device has no `TIMESTAMP_QUERY`: CPU scopes only.
    gpu: Option<GpuProfiler>,
    cpu: HashMap<&'static str, Stage>,
}

/// Accumulated time for one label.
#[derive(Clone, Debug, Default)]
pub struct Stage {
    pub label: String,
    pub count: u32,
    pub total: Duration,
    pub max: Duration,
}

impl Stage {
    fn add(&mut self, elapsed: Duration) {
        self.count += 1;
        self.total += elapsed;
        self.max = self.max.max(elapsed);
    }
}

/// Everything recorded since [`enable`] or the previous report, each list
/// sorted by total time, largest first.
#[derive(Debug, Default)]
pub struct Report {
    /// Per pass label. Summed over passes, so the total is GPU busy time.
    pub gpu: Vec<Stage>,
    /// Per scope label. Scopes on worker threads (decodes) overlap the main
    /// thread, so these do not sum to wall time.
    pub cpu: Vec<Stage>,
    /// The device supports timestamp queries; `gpu` is empty otherwise.
    pub gpu_supported: bool,
}

/// Start recording. Idempotent; the profiler binds to `ctx`'s device.
pub fn enable(ctx: &GpuContext) {
    let mut state = lock();
    if state.is_none() {
        let gpu = ctx.timestamp_queries.then(|| {
            let settings = GpuProfilerSettings { enable_debug_groups: false, ..Default::default() };
            GpuProfiler::new(&ctx.device, settings).expect("valid profiler settings")
        });
        *state = Some(State { gpu, cpu: HashMap::new() });
    }
    ENABLED.store(true, Ordering::Relaxed);
}

fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Timestamp writes for one compute pass. Begin before the pass, hand
/// [`Self::compute`] to its descriptor, and [`Self::end`] on the same
/// encoder once the pass is dropped.
#[must_use]
pub(crate) struct PassTimer(Option<GpuProfilerQuery>);

impl PassTimer {
    pub(crate) fn compute(&self) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        self.0.as_ref()?.compute_pass_timestamp_writes()
    }

    pub(crate) fn end(self, encoder: &mut wgpu::CommandEncoder) {
        let Some(query) = self.0 else { return };
        if let Some(gpu) = lock().as_ref().and_then(|s| s.gpu.as_ref()) {
            gpu.end_query(encoder, query);
        }
    }
}

pub(crate) fn begin_pass(encoder: &mut wgpu::CommandEncoder, label: &str) -> PassTimer {
    if !is_enabled() {
        return PassTimer(None);
    }
    PassTimer(
        lock()
            .as_ref()
            .and_then(|s| s.gpu.as_ref())
            .map(|gpu| gpu.begin_pass_query(label, encoder)),
    )
}

/// Times its own lifetime under `label` when profiling is on.
#[must_use]
pub struct CpuScope(Option<(&'static str, Instant)>);

pub fn cpu_scope(label: &'static str) -> CpuScope {
    CpuScope(is_enabled().then(|| (label, Instant::now())))
}

impl Drop for CpuScope {
    fn drop(&mut self) {
        let Some((label, start)) = self.0 else { return };
        let elapsed = start.elapsed();
        if let Some(state) = lock().as_mut() {
            state
                .cpu
                .entry(label)
                .or_insert_with(|| Stage { label: label.into(), ..Default::default() })
                .add(elapsed);
        }
    }
}

/// Resolve every query recorded so far (one extra submission, then a wait)
/// and return the totals; recording carries on into the next report.
pub fn take_report(ctx: &GpuContext) -> Report {
    let mut guard = lock();
    let Some(state) = guard.as_mut() else { return Report::default() };
    let mut report = Report { gpu_supported: state.gpu.is_some(), ..Default::default() };

    if let Some(gpu) = state.gpu.as_mut() {
        let mut encoder = ctx.encoder("profile_resolve");
        gpu.resolve_queries(&mut encoder);
        ctx.submit(encoder);
        if let Err(e) = gpu.end_frame() {
            tracing::warn!("profiler frame dropped: {e}");
        }
        let period = ctx.queue.get_timestamp_period();
        // The wait runs the map callbacks, so the frame is normally ready
        // after one poll; a couple of retries cover a late callback.
        let mut results = None;
        for _ in 0..3 {
            let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
            results = gpu.process_finished_frame(period);
            if results.is_some() {
                break;
            }
        }
        if results.is_none() {
            tracing::warn!("profiler results not ready; GPU table omitted");
        }
        let mut stages = HashMap::<String, Stage>::new();
        collect(results.as_deref().unwrap_or_default(), &mut stages);
        report.gpu = sorted(stages.into_values());
    }
    report.cpu = sorted(std::mem::take(&mut state.cpu).into_values());
    report
}

fn collect(results: &[GpuTimerQueryResult], stages: &mut HashMap<String, Stage>) {
    for result in results {
        if let Some(time) = &result.time {
            stages
                .entry(result.label.clone())
                .or_insert_with(|| Stage { label: result.label.clone(), ..Default::default() })
                .add(Duration::from_secs_f64((time.end - time.start).max(0.0)));
        }
        collect(&result.nested_queries, stages);
    }
}

fn sorted(stages: impl Iterator<Item = Stage>) -> Vec<Stage> {
    let mut stages: Vec<Stage> = stages.collect();
    stages.sort_by_key(|s| std::cmp::Reverse(s.total));
    stages
}

fn lock() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}
