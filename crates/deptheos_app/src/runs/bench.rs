//! Shared in-app and CLI benchmark runner.
//!
//! Times whole stacks on the live app path (reading frames from disk,
//! decoding, aligning, stacking, and generating the viewer image), with a
//! two-frame warm-up pass and configurable repeats and cooldowns. Produces
//! the exact JSON record shape read by `web/measured/benchmarks/`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use deptheos_aligner::{AlignMode, GridConfig};
use deptheos_core::{Cancel, CoreError};
use deptheos_gpu::GpuContext;
use serde::Serialize;

use crate::engine::{Engine, LIVE_PREVIEW_SIDE};
use crate::library::export::ExportFormat;
use crate::runs::session::{AlignSettings, Alignment, SessionEvent, SessionOptions};
use crate::runs::stacker::StackerKind;
use crate::{Error, Result};

/// How to run a benchmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BenchConfig {
    pub stacker: StackerKind,
    pub align: AlignMode,
    /// Number of timed repeats (median reported).
    pub repeats: u32,
    /// Untimed cooldown seconds before each timed run.
    pub cool_secs: u64,
    /// Time the live app path (live preview readback + viewer image).
    pub live: bool,
}

impl BenchConfig {
    /// Fast in-app default: Pyramid + ECC, 3 timed runs, 5 s cooldown.
    pub fn quick() -> Self {
        Self {
            stacker: StackerKind::Pyramid,
            align: AlignMode::Ecc,
            repeats: 3,
            cool_secs: 5,
            live: true,
        }
    }

    /// Publication-grade run matching `deptheos bench --live --repeat 5 --cool 30`.
    pub fn thorough() -> Self {
        Self {
            stacker: StackerKind::Pyramid,
            align: AlignMode::Ecc,
            repeats: 5,
            cool_secs: 30,
            live: true,
        }
    }
}

/// Live progress reported while a benchmark runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BenchProgress {
    /// Warming up GPU shaders and aligner pools on 2 frames.
    Warmup,
    /// Waiting `seconds_left` seconds before timed round `round` of `total_rounds`.
    Cooldown { round: u32, total_rounds: u32, seconds_left: u64 },
    /// Timed round `round` of `total_rounds`, currently on `frame` of `frames`.
    Running { round: u32, total_rounds: u32, frame: u32, frames: u32 },
}

/// Machine metadata recorded in the benchmark JSON.
#[derive(Clone, Debug, Serialize)]
pub struct MachineInfo {
    pub os: String,
    pub cpu: String,
    pub threads: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<f64>,
    pub gpu: String,
    pub gpu_backend: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gpu_driver: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_cores: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_mode: Option<String>,
}

impl MachineInfo {
    /// Default short label for tables and summaries.
    pub fn default_label(&self) -> String {
        match self.gpu_cores {
            Some(cores) => format!("{} ({cores}-core GPU)", self.cpu),
            None if self.cpu == self.gpu || self.gpu.is_empty() => self.cpu.clone(),
            None => format!("{} + {}", self.cpu, self.gpu),
        }
    }

    /// Detect machine metadata from the current host and GPU context.
    pub fn detect(ctx: &GpuContext) -> Self {
        let info = &ctx.adapter_info;
        let (os, cpu, memory, gpu_cores) = detect_platform();
        let (power, power_mode) = detect_power();
        Self {
            os,
            cpu,
            threads: std::thread::available_parallelism().map_or(0, |n| n.get()),
            memory_gb: memory.map(|b| (b as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0),
            gpu: info.name.clone(),
            gpu_backend: format!("{:?}", info.backend),
            gpu_driver: [info.driver.as_str(), info.driver_info.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
            gpu_cores,
            power,
            power_mode,
        }
    }
}

/// Stack metadata recorded in the benchmark JSON.
#[derive(Clone, Debug, Serialize)]
pub struct StackBenchInfo {
    pub name: String,
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub megapixels: f64,
    pub format: String,
    pub bytes: u64,
}

/// Wall time per phase of one run.
#[derive(Clone, Debug, Default, Serialize)]
pub struct BenchPhases {
    pub pass1: f64,
    pub finalize: f64,
    pub blend: f64,
    pub complete: f64,
}

/// Summarized timing result across all repeats.
#[derive(Clone, Debug, Serialize)]
pub struct RunBenchResult {
    pub seconds: f64,
    pub fastest: f64,
    pub slowest: f64,
    pub mpx_per_s: f64,
    pub ms_per_frame: f64,
    pub phases: BenchPhases,
    pub repeats: Vec<f64>,
    pub reversed: bool,
    pub left_out: usize,
    pub result: [u32; 2],
}

/// Complete benchmark report, ready for UI display, clipboard summary, or JSON export.
#[derive(Clone, Debug)]
pub struct BenchReport {
    pub version: String,
    pub commit: Option<String>,
    pub date: String,
    pub label: String,
    pub machine: MachineInfo,
    pub stack: StackBenchInfo,
    pub path: &'static str,
    pub stacker: StackerKind,
    pub align: AlignMode,
    pub run: RunBenchResult,
}

#[derive(Serialize)]
struct JsonRecord<'a> {
    version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: &'a Option<String>,
    date: &'a str,
    label: &'a str,
    machine: &'a MachineInfo,
    stack: &'a StackBenchInfo,
    path: &'static str,
    runs: serde_json::Map<String, serde_json::Value>,
}

impl BenchReport {
    /// Serialize in the exact JSON shape used by `web/measured/benchmarks/`.
    pub fn to_json(&self) -> String {
        let mut by_align = serde_json::Map::new();
        by_align.insert(
            align_key(self.align).into(),
            serde_json::to_value(&self.run).expect("serializable"),
        );
        let mut runs = serde_json::Map::new();
        runs.insert(self.stacker.key().into(), serde_json::Value::Object(by_align));
        let record = JsonRecord {
            version: &self.version,
            commit: &self.commit,
            date: &self.date,
            label: &self.label,
            machine: &self.machine,
            stack: &self.stack,
            path: self.path,
            runs,
        };
        serde_json::to_string_pretty(&record).expect("serializable") + "\n"
    }

    /// Human-readable summary suitable for pasting into GitHub Discussions/Issues.
    pub fn to_markdown(&self) -> String {
        let s = &self.stack;
        let m = &self.machine;
        let r = &self.run;
        let mem = m.memory_gb.map(|g| format!(" · {g:.0} GB")).unwrap_or_default();
        let commit = self.commit.as_ref().map(|c| format!(" ({c})")).unwrap_or_default();
        format!(
            "#### {}: {} frames of {} × {} {} ({:.1} MP), {}\n\n\
             {} · {} ({}) · {} threads{} · {} · {} · deptheos {}{}\n\n\
             | Method | Alignment | Median | Range | MP/s | ms/frame |\n\
             |---|---|---:|---:|---:|---:|\n\
             | {} | {} | {:.2} s | {:.2}–{:.2} s | {:.0} | {:.1} |\n",
            s.name,
            s.frames,
            s.width,
            s.height,
            s.format,
            s.megapixels,
            self.label,
            m.cpu,
            m.gpu,
            m.gpu_backend,
            m.threads,
            mem,
            m.os,
            self.date,
            self.version,
            commit,
            self.stacker.english_name(),
            align_title(self.align),
            r.seconds,
            r.fastest,
            r.slowest,
            r.mpx_per_s,
            r.ms_per_frame,
        )
    }
}

/// Run a live-path benchmark over `paths` with warm-up, repeats, and cooldowns.
#[allow(clippy::too_many_arguments)]
pub fn run_bench(
    engine: &Engine,
    name: String,
    paths: &[PathBuf],
    config: BenchConfig,
    machine: MachineInfo,
    label: Option<String>,
    cancel: &Cancel,
    on_progress: &mut dyn FnMut(BenchProgress),
) -> Result<BenchReport> {
    if paths.len() < 2 {
        return Err(Error::InvalidState(format!(
            "A benchmark needs at least 2 frames (got {}).",
            paths.len()
        )));
    }
    let repeats = config.repeats.max(1);
    let stack = inspect_stack(name, paths)?;
    let options = SessionOptions {
        alignment: align_settings(config.align),
        live_map: config.live,
        ..Default::default()
    };
    let stacker_config = config.stacker.default_config();

    // Warm up shaders and aligner pools on 2 frames (untimed).
    on_progress(BenchProgress::Warmup);
    time_one(engine, &paths[..2], stacker_config, options, config.live, cancel, &mut |_, _| {})?;

    let mut runs: Vec<(f64, BenchPhases)> = Vec::with_capacity(repeats as usize);
    let mut geometry = Geometry { reversed: false, left_out: 0, size: [0, 0] };

    for round in 1..=repeats {
        if config.cool_secs > 0 {
            sleep_cooldown(config.cool_secs, round, repeats, cancel, on_progress)?;
        }
        let total_frames = paths.len() as u32;
        on_progress(BenchProgress::Running {
            round,
            total_rounds: repeats,
            frame: 0,
            frames: total_frames,
        });
        let (wall, phases, geom) = time_one(
            engine,
            paths,
            stacker_config,
            options,
            config.live,
            cancel,
            &mut |frame, frames| {
                on_progress(BenchProgress::Running { round, total_rounds: repeats, frame, frames });
            },
        )?;
        if round == 1 {
            geometry = geom;
        }
        runs.push((wall, phases));
    }

    let run = summarize(&stack, runs, geometry);
    let label = label.unwrap_or_else(|| machine.default_label());
    Ok(BenchReport {
        version: env!("CARGO_PKG_VERSION").into(),
        commit: option_env!("DEPTHEOS_GIT_COMMIT").map(str::to_string),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        label,
        machine,
        stack,
        path: if config.live { "live" } else { "session" },
        stacker: config.stacker,
        align: config.align,
        run,
    })
}

fn sleep_cooldown(
    cool_secs: u64,
    round: u32,
    total_rounds: u32,
    cancel: &Cancel,
    on_progress: &mut dyn FnMut(BenchProgress),
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(cool_secs);
    let mut last_reported = u64::MAX;
    loop {
        if cancel.is_canceled() {
            return Err(Error::Core(CoreError::Canceled));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }
        let left = (deadline - now).as_secs() + 1;
        if left != last_reported {
            last_reported = left;
            on_progress(BenchProgress::Cooldown { round, total_rounds, seconds_left: left });
        }
        let step = (deadline - now).min(Duration::from_millis(200));
        std::thread::sleep(step);
    }
}

#[derive(Clone, Copy)]
struct Geometry {
    reversed: bool,
    left_out: usize,
    size: [u32; 2],
}

fn time_one(
    engine: &Engine,
    paths: &[PathBuf],
    config: crate::StackerConfig,
    options: SessionOptions,
    live: bool,
    cancel: &Cancel,
    on_frame: &mut dyn FnMut(u32, u32),
) -> Result<(f64, BenchPhases, Geometry)> {
    let start = Instant::now();
    let mut phases = BenchPhases::default();
    let mut last = start;
    let total = paths.len() as u32;
    let mut done_frames = 0u32;
    let mut mark = |event: SessionEvent| {
        let now = Instant::now();
        let step = (now - last).as_secs_f64();
        match event {
            SessionEvent::Started { .. } => {
                done_frames = 0;
            }
            SessionEvent::Accumulated { .. } => {
                phases.pass1 += step;
                done_frames = (done_frames + 1).min(total);
                on_frame(done_frames, total);
            }
            SessionEvent::Reversed => {
                phases.pass1 += step;
                done_frames = 0;
            }
            SessionEvent::Finalized => phases.finalize += step,
            SessionEvent::Blended { .. } => {
                phases.blend += step;
                done_frames = (done_frames + 1).min(total);
                on_frame(done_frames, total);
            }
            SessionEvent::Completed => phases.complete += step,
        }
        last = now;
    };
    let stack = if live {
        engine
            .stack(paths.to_vec(), config, options, LIVE_PREVIEW_SIDE, cancel, &mut mark, |_| {})?
            .stack
    } else {
        let session = engine.session(paths.to_vec(), config, options)?;
        session.run(&mut |event, _| mark(event), cancel)?.stack
    };
    let wall = start.elapsed().as_secs_f64();
    let geom = Geometry {
        reversed: stack.geometry.reversed,
        left_out: stack.geometry.left_out.len(),
        size: [stack.width, stack.height],
    };
    Ok((wall, phases, geom))
}

fn inspect_stack(name: String, paths: &[PathBuf]) -> Result<StackBenchInfo> {
    let first = &paths[0];
    let (width, height) = if let Some(header) = crate::header_info(first, crate::CameraRaw::LibRaw)
    {
        (header.width, header.height)
    } else {
        let image = crate::load_image(first, crate::CameraRaw::LibRaw)?;
        (image.width(), image.height())
    };
    let format = match (ExportFormat::of(first), first.extension().and_then(|e| e.to_str())) {
        (Some(f), _) => f.label().into(),
        (None, Some(e)) => e.to_ascii_uppercase(),
        (None, None) => "?".into(),
    };
    let bytes = paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    Ok(StackBenchInfo {
        name,
        frames: paths.len(),
        width,
        height,
        megapixels: f64::from(width) * f64::from(height) / 1e6,
        format,
        bytes,
    })
}

fn summarize(
    stack: &StackBenchInfo,
    runs: Vec<(f64, BenchPhases)>,
    geometry: Geometry,
) -> RunBenchResult {
    let mut order: Vec<usize> = (0..runs.len()).collect();
    order.sort_by(|&a, &b| runs[a].0.total_cmp(&runs[b].0));
    let n = order.len();
    let lower = order[(n - 1) / 2];
    let seconds =
        if n % 2 == 1 { runs[lower].0 } else { (runs[lower].0 + runs[order[n / 2]].0) / 2.0 };
    let mpx = stack.megapixels * stack.frames as f64;
    RunBenchResult {
        seconds,
        fastest: runs[order[0]].0,
        slowest: runs[order[n - 1]].0,
        mpx_per_s: mpx / seconds,
        ms_per_frame: 1e3 * seconds / stack.frames as f64,
        phases: runs[lower].1.clone(),
        repeats: runs.iter().map(|r| r.0).collect(),
        reversed: geometry.reversed,
        left_out: geometry.left_out,
        result: geometry.size,
    }
}

fn align_settings(mode: AlignMode) -> Alignment {
    Alignment::On(AlignSettings { mode, grid: GridConfig::default(), crop: true })
}

fn align_key(mode: AlignMode) -> &'static str {
    match mode {
        AlignMode::Ecc => "ecc",
        AlignMode::Grid => "grid",
    }
}

fn align_title(mode: AlignMode) -> &'static str {
    match mode {
        AlignMode::Ecc => "ECC",
        AlignMode::Grid => "Grid search",
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn sh(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn detect_platform() -> (String, String, Option<u64>, Option<u32>) {
    let os = format!("macOS {}", sh("sw_vers", &["-productVersion"]).unwrap_or_default());
    let cpu = sh("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_else(|| "?".into());
    let memory = sh("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.parse().ok());
    let cores = sh("system_profiler", &["SPDisplaysDataType"]).and_then(|s| {
        s.lines()
            .find_map(|l| l.trim().strip_prefix("Total Number of Cores:"))
            .and_then(|n| n.trim().parse().ok())
    });
    (os, cpu, memory, cores)
}

#[cfg(target_os = "windows")]
fn detect_platform() -> (String, String, Option<u64>, Option<u32>) {
    let reg = |key: &str, value: &str| {
        sh("reg", &["query", key, "/v", value])
            .and_then(|s| s.split("REG_").nth(1).map(str::to_string))
            .and_then(|s| s.split_once(char::is_whitespace).map(|(_, v)| v.trim().to_string()))
    };
    let version = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build: u32 = reg(version, "CurrentBuild").and_then(|b| b.parse().ok()).unwrap_or(0);
    let edition = reg(version, "ProductName").map(|p| {
        let p = p.strip_prefix("Windows 10").or_else(|| p.strip_prefix("Windows 11")).unwrap_or("");
        p.trim().to_string()
    });
    let os = [
        Some(if build >= 22000 { "Windows 11" } else { "Windows 10" }.to_string()),
        edition.filter(|e| !e.is_empty()),
        reg(version, "DisplayVersion"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let cpu = reg(r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
        .unwrap_or_else(|| "?".into());
    let memory = sh(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_PhysicalMemory | Measure-Object Capacity -Sum).Sum",
        ],
    )
    .and_then(|s| s.parse().ok());
    (os, cpu, memory, None)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn detect_platform() -> (String, String, Option<u64>, Option<u32>) {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| std::env::consts::OS.into());
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.into());
    let memory = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        s.lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    });
    (os, cpu, memory, None)
}

#[cfg(target_os = "macos")]
fn detect_power() -> (Option<String>, Option<String>) {
    let source = sh("pmset", &["-g", "batt"]).and_then(|s| {
        let line = s.lines().next()?;
        Some(line.split('\'').nth(1)?.to_string())
    });
    let mode = sh("pmset", &["-g"]).and_then(|s| {
        let value = s.lines().find_map(|l| {
            let l = l.trim();
            l.strip_prefix("powermode").or_else(|| l.strip_prefix("lowpowermode"))
        })?;
        match value.trim() {
            "0" => Some("automatic".into()),
            "1" => Some("low power".into()),
            "2" => Some("high power".into()),
            _ => None,
        }
    });
    (source, mode)
}

#[cfg(not(target_os = "macos"))]
fn detect_power() -> (Option<String>, Option<String>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_computes_median_and_throughput() {
        let stack = StackBenchInfo {
            name: "pcb".into(),
            frames: 200,
            width: 4096,
            height: 2732,
            megapixels: 11.190272,
            format: "JPEG".into(),
            bytes: 100_000,
        };
        let runs = vec![
            (3.0, BenchPhases::default()),
            (2.0, BenchPhases::default()),
            (4.0, BenchPhases::default()),
        ];
        let geom = Geometry { reversed: false, left_out: 0, size: [4096, 2732] };
        let res = summarize(&stack, runs, geom);
        assert!((res.seconds - 3.0).abs() < 1e-9);
        assert!((res.fastest - 2.0).abs() < 1e-9);
        assert!((res.slowest - 4.0).abs() < 1e-9);
        assert!((res.ms_per_frame - 15.0).abs() < 1e-9);
        assert!((res.mpx_per_s - (200.0 * 11.190272 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn report_json_matches_web_measured_schema() {
        let report = BenchReport {
            version: "0.2.0".into(),
            commit: Some("abc1234".into()),
            date: "2026-11-22".into(),
            label: "iPad Pro (M5)".into(),
            machine: MachineInfo {
                os: "iPadOS 27.2".into(),
                cpu: "Apple M5".into(),
                threads: 10,
                memory_gb: Some(16.0),
                gpu: "Apple M5".into(),
                gpu_backend: "Metal".into(),
                gpu_driver: String::new(),
                gpu_cores: None,
                power: Some("AC Power".into()),
                power_mode: Some("automatic".into()),
            },
            stack: StackBenchInfo {
                name: "hornet".into(),
                frames: 66,
                width: 4096,
                height: 2732,
                megapixels: 11.190272,
                format: "JPEG".into(),
                bytes: 42_000_000,
            },
            path: "live",
            stacker: StackerKind::Pyramid,
            align: AlignMode::Ecc,
            run: RunBenchResult {
                seconds: 1.5,
                fastest: 1.4,
                slowest: 1.6,
                mpx_per_s: 492.37,
                ms_per_frame: 22.7,
                phases: BenchPhases::default(),
                repeats: vec![1.5, 1.4, 1.6],
                reversed: false,
                left_out: 0,
                result: [4096, 2732],
            },
        };
        let v: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(v["path"], "live");
        assert_eq!(v["stack"]["frames"], 66);
        assert_eq!(v["runs"]["pyramid"]["ecc"]["seconds"], 1.5);
    }

    #[test]
    fn cooldown_exits_immediately_when_cancelled() {
        let cancel = Cancel::root();
        cancel.cancel();
        let err = sleep_cooldown(30, 1, 5, &cancel, &mut |_| {}).unwrap_err();
        assert!(matches!(err, Error::Core(CoreError::Canceled)));
    }
}
