//! In-app benchmark bridge (`deptheos_app::runs::bench`).

use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::runs::bench::{self, BenchConfig, BenchProgress, MachineInfo};

use crate::request::on_thread;
use crate::{Engine, EngineError, Request, Result};

/// Device metadata supplied by the iOS / iPadOS host.
#[derive(Clone, Debug, uniffi::Record)]
pub struct BenchMachine {
    pub label: String,
    pub os: String,
    pub cpu: String,
    pub threads: u32,
    pub memory_gb: Option<f64>,
    pub power: Option<String>,
    pub power_mode: Option<String>,
}

/// Progress update emitted during a benchmark run.
#[derive(Clone, Debug, uniffi::Enum)]
pub enum BenchUpdate {
    Warmup,
    Cooldown { round: u32, total_rounds: u32, seconds_left: u64 },
    Running { round: u32, total_rounds: u32, frame: u32, frames: u32 },
}

/// Receives live progress updates while [`Engine::run_benchmark`] runs.
#[uniffi::export(foreign)]
pub trait BenchObserver: Send + Sync {
    fn progress(&self, update: BenchUpdate);
}

/// Completed benchmark result for display, copy, and JSON export.
#[derive(Clone, Debug, uniffi::Record)]
pub struct BenchSummary {
    pub stack_name: String,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub megapixels: f64,
    pub format: String,
    pub label: String,
    pub seconds: f64,
    pub fastest: f64,
    pub slowest: f64,
    pub mpx_per_s: f64,
    pub ms_per_frame: f64,
    pub repeats: Vec<f64>,
    pub left_out: u32,
    pub json: String,
    pub markdown: String,
}

#[uniffi::export]
impl Engine {
    /// Run a live-path Pyramid + ECC benchmark over `paths` on a worker thread.
    /// Cancelable at any time (including during cooldown) via `request`.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_benchmark(
        &self,
        name: String,
        paths: Vec<PathBuf>,
        repeats: u32,
        cool_secs: u64,
        machine: BenchMachine,
        request: Arc<Request>,
        observer: Arc<dyn BenchObserver>,
    ) -> Result<BenchSummary> {
        let engine = self.engine.clone();
        let config = BenchConfig { repeats: repeats.max(1), cool_secs, ..BenchConfig::quick() };
        on_thread(self.engine.words(), "benchmark", &request, move |cancel| {
            let gpu_info = &engine.gpu().adapter_info;
            let host = MachineInfo {
                os: machine.os,
                cpu: machine.cpu,
                threads: machine.threads as usize,
                memory_gb: machine.memory_gb,
                gpu: gpu_info.name.clone(),
                gpu_backend: format!("{:?}", gpu_info.backend),
                gpu_driver: String::new(),
                gpu_cores: None,
                power: machine.power,
                power_mode: machine.power_mode,
            };
            let label = (!machine.label.is_empty()).then_some(machine.label);
            let mut notify = |p: BenchProgress| {
                let update = match p {
                    BenchProgress::Warmup => BenchUpdate::Warmup,
                    BenchProgress::Cooldown { round, total_rounds, seconds_left } => {
                        BenchUpdate::Cooldown { round, total_rounds, seconds_left }
                    }
                    BenchProgress::Running { round, total_rounds, frame, frames } => {
                        BenchUpdate::Running { round, total_rounds, frame, frames }
                    }
                };
                observer.progress(update);
            };
            let report =
                bench::run_bench(&engine, name, &paths, config, host, label, cancel, &mut notify)
                    .map_err(|e| EngineError::explained(&e, engine.words()))?;
            let json = report.to_json();
            let markdown = report.to_markdown();
            Ok(BenchSummary {
                stack_name: report.stack.name,
                frames: report.stack.frames as u32,
                width: report.stack.width,
                height: report.stack.height,
                megapixels: report.stack.megapixels,
                format: report.stack.format,
                label: report.label,
                seconds: report.run.seconds,
                fastest: report.run.fastest,
                slowest: report.run.slowest,
                mpx_per_s: report.run.mpx_per_s,
                ms_per_frame: report.run.ms_per_frame,
                repeats: report.run.repeats,
                left_out: report.run.left_out as u32,
                json,
                markdown,
            })
        })
        .await
    }

    /// What Highlights and Shadows cost in memory on this device
    /// (`deptheos_app::runs::probe`): `draw` is `without`, `whole` or
    /// `bands`, on a synthetic `width`×`height` stack, bands of `band`
    /// rows. One draw per launch. The line it returns is also logged.
    pub async fn probe_local_tone(
        &self,
        draw: String,
        width: u32,
        height: u32,
        band: u32,
        request: Arc<Request>,
    ) -> Result<String> {
        use deptheos_app::runs::probe::{Draw, local_tone_memory};
        let engine = self.engine.clone();
        on_thread(self.engine.words(), "local tone probe", &request, move |_| {
            let Some(which) = Draw::named(&draw, band) else {
                return Err(EngineError::Failed { message: format!("no draw named {draw}") });
            };
            let left = deptheos_app::engine::memory::left();
            let step = local_tone_memory(engine.gpu(), width, height, which)
                .map_err(|e| EngineError::explained(&e, engine.words()))?;
            Ok(format!(
                "local tone probe, {width}x{height}: {} in {} ms, footprint {} MB, peak {} MB, after {} MB; {} MB left at the start",
                step.what,
                step.millis,
                step.before >> 20,
                step.peak >> 20,
                step.after >> 20,
                left.map_or("?".into(), |b| (b >> 20).to_string()),
            ))
        })
        .await
    }
}
