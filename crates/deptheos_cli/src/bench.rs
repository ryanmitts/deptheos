//! `deptheos bench`: time whole stacks over every chosen stacker and
//! aligner on one set of frames, and report the machine with the numbers.
//!
//! A run is timed from the session's creation to its result in CPU memory;
//! saving is not timed. The device, pipelines and a warm-up stack per
//! stacker come first, so the numbers are an already-open app's. With
//! `--repeat`, each round times every configuration once, so drift (heat,
//! the file cache) falls on all of them alike; each reports its median.
//! DNG conversion (`--raw-backend adobe-dng`) runs before timing and is
//! reported on its own.
//!
//! The table on stdout is Markdown; `--json` writes the full record in the
//! shape the site reads from `web/measured/benchmarks/`. `--profile` prints
//! GPU and CPU time per stage (`deptheos_gpu::profile`); `--save-raw` and
//! `--compare-raw` check a kernel change bit for bit. Run under
//! `/usr/bin/time -l` for peak memory.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use clap::{Args, ValueEnum};
use deptheos_app::library::export::ExportFormat;
use deptheos_app::{
    CoreError, Engine, GpuContext, GridConfig, LIVE_PREVIEW_SIDE, SaveOptions, SessionEvent,
    SessionOptions, StackerConfig, StackerKind,
};
use deptheos_gpu::profile::{self, Stage};
use serde::Serialize;

use crate::args::{Aligner, Checks, DecodeArgs};
use crate::error::{Refusal, Result};
use crate::prepare;
use crate::ui::progress;

#[derive(Args)]
pub struct BenchArgs {
    /// Frames, or a folder of them
    #[arg(required = true)]
    inputs: Vec<PathBuf>,

    /// Methods to time (comma-separated)
    #[arg(long, value_enum, value_delimiter = ',', default_value = "pyramid,depth")]
    stacker: Vec<Method>,

    /// Aligners to time (comma-separated)
    #[arg(short, long, value_enum, value_delimiter = ',', default_value = "ecc,grid,none")]
    align: Vec<Aligner>,

    /// Time each configuration this many times, one round of all of them
    /// after another; the table reports the median and the JSON keeps
    /// every time
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=20))]
    repeat: u32,

    /// Wait this many seconds before each timed run, untimed, so a laptop
    /// cools between stacks as it would between a photographer's
    #[arg(long, value_name = "SECONDS", default_value_t = 0)]
    cool: u64,

    /// Time the stack as the app runs it (with the live preview read back
    /// while stacking, and the picture for the viewer made at the end)
    /// instead of as the other commands do
    #[arg(long)]
    live: bool,

    /// Also write each configuration's result into this folder, as
    /// <method>-<aligner>.jpg (not timed)
    #[arg(short, long, value_name = "DIR")]
    output: Option<PathBuf>,

    /// Write the full record as JSON here (the machine, the frames, every
    /// run with its phases and repeats)
    #[arg(long, value_name = "PATH")]
    json: Option<PathBuf>,

    /// A short name for this machine in the record and the table
    /// [default: the CPU and GPU, or on a Mac the chip and its GPU core
    /// count]
    #[arg(long, value_name = "NAME")]
    machine: Option<String>,

    /// Print where each run's time went on stderr: GPU time per pass
    /// (where the GPU has timestamp queries) and CPU time per stage
    #[arg(long)]
    profile: bool,

    /// Also write each configuration's result into this folder as its raw
    /// RGB samples, 16-bit little-endian, as <method>-<aligner>.u16 (not
    /// timed), for --compare-raw
    #[arg(long, value_name = "DIR")]
    save_raw: Option<PathBuf>,

    /// Compare each configuration's result with its <method>-<aligner>.u16
    /// in this folder, written by --save-raw: how many samples differ and
    /// by how much
    #[arg(long, value_name = "DIR")]
    compare_raw: Option<PathBuf>,

    #[command(flatten)]
    decode: DecodeArgs,
}

impl BenchArgs {
    pub(crate) fn check(&self, c: &mut Checks) {
        self.decode.check(c);
    }
}

/// A stacker, as `--stacker` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Method {
    Pyramid,
    Depth,
    Wavelet,
}

impl From<Method> for StackerKind {
    fn from(m: Method) -> Self {
        match m {
            Method::Pyramid => StackerKind::Pyramid,
            Method::Depth => StackerKind::Depth,
            Method::Wavelet => StackerKind::Wavelet,
        }
    }
}

/// How the table and the record title an aligner.
fn align_title(align: Aligner) -> &'static str {
    match align {
        Aligner::Ecc => "ECC",
        Aligner::Grid => "Grid search",
        Aligner::None => "None",
    }
}

/// The whole record, as `--json` writes it.
#[derive(Serialize)]
struct Record {
    /// The CLI's version, and the commit it was built from.
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    /// The day, as YYYY-MM-DD.
    date: String,
    label: String,
    machine: Machine,
    stack: StackInfo,
    /// "session" (the CLI's path) or "live" (the app's, with its preview).
    path: &'static str,
    /// Keyed by stacker, then aligner.
    runs: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct Machine {
    os: String,
    cpu: String,
    threads: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_gb: Option<f64>,
    gpu: String,
    gpu_backend: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    gpu_driver: String,
    /// Apple silicon: the GPU core count, from `system_profiler`.
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu_cores: Option<u32>,
    /// Where the power came from, as the OS says it ("AC Power", "Battery
    /// Power"); macOS only.
    #[serde(skip_serializing_if = "Option::is_none")]
    power: Option<String>,
    /// macOS's energy mode: "automatic", "low power" or "high power".
    #[serde(skip_serializing_if = "Option::is_none")]
    power_mode: Option<&'static str>,
}

#[derive(Serialize)]
struct StackInfo {
    /// The folder's name, or the first frame's parent folder.
    name: String,
    frames: usize,
    width: u32,
    height: u32,
    /// Per frame.
    megapixels: f64,
    /// Of the frames timed: JPEG, DNG, …
    format: String,
    /// When `--raw-backend adobe-dng` converted them: what they were.
    #[serde(skip_serializing_if = "Option::is_none")]
    converted_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversion_seconds: Option<f64>,
    /// The conversions were written uncompressed (`--dng-uncompressed`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    uncompressed: bool,
    /// Camera RAW: "LibRaw" or "Adobe DNG Converter".
    #[serde(skip_serializing_if = "Option::is_none")]
    decoder: Option<&'static str>,
    /// LibRaw's camera profile preference (`--camera-profile`).
    #[serde(skip_serializing_if = "Option::is_none")]
    camera_profile: Option<String>,
    /// Total bytes of the frames timed, on disk.
    bytes: u64,
}

#[derive(Clone, Serialize)]
struct RunResult {
    /// The median repeat (of an even count, the mean of the middle two).
    seconds: f64,
    fastest: f64,
    slowest: f64,
    /// From `seconds`.
    mpx_per_s: f64,
    ms_per_frame: f64,
    /// Where the median repeat's time went (of an even count, the faster
    /// middle one's).
    phases: Phases,
    /// Every repeat's wall time, in order.
    repeats: Vec<f64>,
    reversed: bool,
    left_out: usize,
    /// The result's size after the crop.
    result: [u32; 2],
}

/// Wall time per phase of one run, from the session's events.
#[derive(Clone, Default, Serialize)]
struct Phases {
    /// Decoding, aligning and accumulating every frame (pass 1; both
    /// attempts when the reverse check restarts it).
    pass1: f64,
    /// Between the passes (the pyramid's collapse, the depth map's
    /// smoothing, the wavelet merge).
    finalize: f64,
    /// The blend pass's second read of the frames; 0 for the pyramid.
    blend: f64,
    /// The blend stackers' last step, then download, crop and the maps.
    complete: f64,
}

pub fn run(args: BenchArgs, words: &deptheos_app::Words) -> Result<()> {
    let given = prepare::frames(&args.inputs)?;
    if given.len() < 2 {
        return Err(Refusal::TooFewFrames(given.len()).into());
    }

    let backend = args.decode.backend();
    let converting = given.iter().any(|p| backend.converts(p));
    let engine = prepare::engine(&args.decode, words)?;
    let ctx = engine.gpu();
    if args.profile {
        profile::enable(ctx);
    }
    let started = Instant::now();
    if converting {
        let jobs = deptheos_app::converter_jobs();
        progress!("converting {} frames to DNG ({jobs} at once), not timed", given.len());
    }
    // Held for the whole run, so every stack reads them from the cache.
    let converted = prepare::convert_first(&engine, &given, backend)?;
    let conversion_seconds = converting.then(|| started.elapsed().as_secs_f64());
    // What the stacks decode: the conversions, where there are any.
    let read: Vec<&Path> = given.iter().map(|p| converted.file(p)).collect();
    let stack = stack_info(&args, &given, &read, conversion_seconds)?;
    let machine = machine(ctx);
    if machine.power.as_deref().is_some_and(|p| p != "AC Power")
        || machine.power_mode == Some("low power")
    {
        progress!(
            "warning: on battery or in Low Power Mode, so the times are not the machine's best"
        );
    }
    let label = args.machine.clone().unwrap_or_else(|| machine.default_label());
    progress!(
        "{}: {} frames of {} × {} {} ({:.1} MP each) on {label}: {} / {} ({:?})",
        stack.name,
        stack.frames,
        stack.width,
        stack.height,
        stack.format,
        stack.megapixels,
        machine.cpu,
        machine.gpu,
        ctx.adapter_info.backend
    );

    let base_options = SessionOptions {
        raw_backend: backend,
        camera_profile: args.decode.profile(),
        ..Default::default()
    };
    let options = |align: Aligner| SessionOptions {
        alignment: align.alignment(true, GridConfig::default()),
        ..base_options
    };

    // Warm up each stacker's shaders and the aligner's pool on two frames.
    for &stacker in &args.stacker {
        let config = StackerKind::from(stacker).default_config();
        time_one(&engine, &given[..2], config, options(args.align[0]), args.live)?;
    }

    let configs: Vec<(StackerKind, Aligner)> = args
        .stacker
        .iter()
        .flat_map(|&m| args.align.iter().map(move |&a| (StackerKind::from(m), a)))
        .collect();
    // Per configuration: every round's wall time and phases, and the
    // first round's geometry (the same every round).
    let mut times: Vec<(Vec<(f64, Phases)>, Geometry)> = Vec::new();
    for round in 1..=args.repeat {
        for (i, &(stacker, align)) in configs.iter().enumerate() {
            if args.cool > 0 {
                std::thread::sleep(Duration::from_secs(args.cool));
            }
            if args.profile {
                // Each run's profile from zero.
                let _ = profile::take_report(ctx);
            }
            let (wall, phases, result) =
                time_one(&engine, &given, stacker.default_config(), options(align), args.live)?;
            let (method, aligner) = (stacker.label(words), align_title(align));
            progress!("{method} + {aligner}, round {round} of {}: {wall:.2} s", args.repeat);
            if args.profile {
                print_profile(&profile::take_report(ctx), wall);
            }
            if round == 1 {
                write_result(&args, &engine, stacker, align, &result)?;
                times.push((Vec::new(), result.geometry));
            }
            times[i].0.push((wall, phases));
        }
    }

    let mut runs = serde_json::Map::new();
    let mut rows: Vec<(StackerKind, Aligner, RunResult)> = Vec::new();
    for (&(stacker, align), (repeats, geometry)) in configs.iter().zip(times) {
        let run = summarize(&stack, repeats, geometry);
        let by_align = runs
            .entry(stacker.key())
            .or_insert_with(|| serde_json::Map::new().into())
            .as_object_mut()
            .expect("an object");
        by_align.insert(align.key().into(), serde_json::to_value(&run).expect("serializable"));
        rows.push((stacker, align, run));
    }

    let record = Record {
        version: env!("CARGO_PKG_VERSION").into(),
        commit: option_env!("DEPTHEOS_GIT_COMMIT").map(str::to_string),
        date: today(),
        label,
        machine,
        stack,
        path: if args.live { "live" } else { "session" },
        runs,
    };
    print_markdown(&record, &rows);
    if let Some(path) = &args.json {
        write_json(path, &record)?;
    }
    Ok(())
}

/// The frames timed: `given` as named, `read` as decoded (their
/// conversions, if any).
fn stack_info(
    args: &BenchArgs,
    given: &[PathBuf],
    read: &[&Path],
    conversion_seconds: Option<f64>,
) -> Result<StackInfo> {
    let converting = conversion_seconds.is_some();
    let decoder = if converting {
        Some("Adobe DNG Converter")
    } else if args.decode.backend().decodes_linear(&given[0]) && !deptheos_app::is_dng(&given[0]) {
        Some("LibRaw")
    } else {
        None
    };
    let (width, height) = frame_size(read[0])?;
    Ok(StackInfo {
        name: stack_name(&args.inputs, given),
        frames: given.len(),
        width,
        height,
        megapixels: f64::from(width) * f64::from(height) / 1e6,
        format: format_name(read[0]),
        converted_from: converting.then(|| format_name(&given[0])),
        conversion_seconds,
        uncompressed: converting && args.decode.dng_uncompressed,
        decoder,
        camera_profile: (decoder == Some("LibRaw"))
            .then(|| args.decode.camera_profile.unwrap_or_default().to_possible_value())
            .flatten()
            .map(|v| v.get_name().to_string()),
        bytes: read.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum(),
    })
}

/// One configuration's record from its runs' times, in order.
fn summarize(stack: &StackInfo, runs: Vec<(f64, Phases)>, geometry: Geometry) -> RunResult {
    let mut order: Vec<usize> = (0..runs.len()).collect();
    order.sort_by(|&a, &b| runs[a].0.total_cmp(&runs[b].0));
    let n = order.len();
    let lower = order[(n - 1) / 2];
    let seconds =
        if n % 2 == 1 { runs[lower].0 } else { (runs[lower].0 + runs[order[n / 2]].0) / 2.0 };
    let mpx = stack.megapixels * stack.frames as f64;
    RunResult {
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

/// A configuration's result written as `--output`, `--save-raw` and
/// `--compare-raw` ask, untimed.
fn write_result(
    args: &BenchArgs,
    engine: &Engine,
    stacker: StackerKind,
    align: Aligner,
    result: &Timed,
) -> Result<()> {
    if let Some(dir) = &args.output {
        std::fs::create_dir_all(dir).map_err(CoreError::from)?;
        let path = dir.join(format!("{}-{}.jpg", stacker.key(), align.key()));
        engine.save(&result.stack, &path, SaveOptions::default())?;
        progress!("wrote {}", path.display());
    }
    let raw_name = format!("{}-{}.u16", stacker.key(), align.key());
    if let Some(dir) = &args.save_raw {
        std::fs::create_dir_all(dir).map_err(CoreError::from)?;
        let path = dir.join(&raw_name);
        let bytes: Vec<u8> = result.stack.pixels.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(&path, bytes).map_err(CoreError::from)?;
        progress!("wrote {}", path.display());
    }
    if let Some(dir) = &args.compare_raw {
        let path = dir.join(&raw_name);
        let reference = std::fs::read(&path).map_err(CoreError::from)?;
        progress!("{}: {}", path.display(), compare_samples(&result.stack.pixels, &reference));
    }
    Ok(())
}

/// `record` as JSON at `path`, its folder made if need be.
fn write_json(path: &Path, record: &Record) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(CoreError::from)?;
    }
    let json = serde_json::to_string_pretty(record).expect("serializable");
    std::fs::write(path, json + "\n").map_err(CoreError::from)?;
    progress!("wrote {}", path.display());
    Ok(())
}

/// What one timed run yields besides its times.
struct Timed {
    stack: deptheos_app::CompletedStack,
    geometry: Geometry,
}

/// How the frames lined up, and the result's size after the crop.
#[derive(Clone, Copy)]
struct Geometry {
    reversed: bool,
    left_out: usize,
    size: [u32; 2],
}

/// Run one stack and time it: wall seconds, the phases, and the result.
fn time_one(
    engine: &Engine,
    paths: &[PathBuf],
    config: StackerConfig,
    options: SessionOptions,
    live: bool,
) -> Result<(f64, Phases, Timed)> {
    let start = Instant::now();
    let mut phases = Phases::default();
    let mut last = start;
    let mut mark = |event: SessionEvent| {
        let now = Instant::now();
        let step = (now - last).as_secs_f64();
        match event {
            SessionEvent::Started { .. } => {}
            SessionEvent::Accumulated { .. } | SessionEvent::Reversed => phases.pass1 += step,
            SessionEvent::Finalized => phases.finalize += step,
            SessionEvent::Blended { .. } => phases.blend += step,
            SessionEvent::Completed => phases.complete += step,
        }
        last = now;
    };
    let cancel = crate::interrupt::token();
    let stack = if live {
        let options = SessionOptions { live_map: true, ..options };
        engine
            .stack(paths.to_vec(), config, options, LIVE_PREVIEW_SIDE, cancel, &mut mark, |_| {})?
            .stack
    } else {
        let session = engine.session(paths.to_vec(), config, options)?;
        session.run(&mut |event, _| mark(event), cancel)?.stack
    };
    let wall = start.elapsed().as_secs_f64();
    let timed = Timed {
        geometry: Geometry {
            reversed: stack.geometry.reversed,
            left_out: stack.geometry.left_out.len(),
            size: [stack.width, stack.height],
        },
        stack,
    };
    Ok((wall, phases, timed))
}

fn print_markdown(record: &Record, rows: &[(StackerKind, Aligner, RunResult)]) {
    let s = &record.stack;
    let m = &record.machine;
    println!(
        "#### {}: {} frames of {} × {} {} ({:.1} MP), {}\n",
        s.name, s.frames, s.width, s.height, s.format, s.megapixels, record.label
    );
    println!(
        "{} · {} ({}) · {} threads{} · {} · {} · deptheos {}{}\n",
        m.cpu,
        m.gpu,
        m.gpu_backend,
        m.threads,
        m.memory_gb.map(|g| format!(" · {g:.0} GB")).unwrap_or_default(),
        m.os,
        record.date,
        record.version,
        record.commit.as_ref().map(|c| format!(" ({c})")).unwrap_or_default()
    );
    println!("| Method | Alignment | Time | Range | MP/s | ms/frame |");
    println!("|---|---|---:|---:|---:|---:|");
    for (stacker, align, run) in rows {
        let note = match (run.reversed, run.left_out) {
            (false, 0) => String::new(),
            (true, 0) => " (reversed)".into(),
            (false, n) => format!(" ({n} left out)"),
            (true, n) => format!(" (reversed, {n} left out)"),
        };
        println!(
            "| {} | {} | {:.1} s{note} | {:.1}–{:.1} s | {:.0} | {:.0} |",
            stacker.english_name(),
            align_title(*align),
            run.seconds,
            run.fastest,
            run.slowest,
            run.mpx_per_s,
            run.ms_per_frame
        );
    }
    if let (Some(from), Some(secs)) = (&s.converted_from, s.conversion_seconds) {
        let how = if s.uncompressed { ", uncompressed" } else { "" };
        println!(
            "\nConverted from {from} with Adobe DNG Converter in {secs:.1} s (not timed{how})."
        );
    }
    println!();
}

/// How far `pixels` is from a `--save-raw` file of the same stack.
fn compare_samples(pixels: &[u16], reference: &[u8]) -> String {
    if reference.len() != pixels.len() * 2 {
        return format!("size differs ({} vs {} samples)", reference.len() / 2, pixels.len());
    }
    let mut differing = 0usize;
    let mut max_diff = 0u16;
    let mut sum_diff = 0u64;
    for (&a, b) in pixels.iter().zip(reference.as_chunks::<2>().0) {
        let d = a.abs_diff(u16::from_le_bytes(*b));
        if d > 0 {
            differing += 1;
            max_diff = max_diff.max(d);
            sum_diff += u64::from(d);
        }
    }
    if differing == 0 {
        format!("identical ({} samples)", pixels.len())
    } else {
        format!(
            "{differing} of {} samples differ ({:.4} %), max {max_diff}/65535, mean |diff| over differing {:.2}",
            pixels.len(),
            100.0 * differing as f64 / pixels.len() as f64,
            sum_diff as f64 / differing as f64
        )
    }
}

/// A run's profile on stderr: GPU passes (busy time against the run's
/// `wall` seconds) and CPU stages.
fn print_profile(report: &profile::Report, wall: f64) {
    if report.gpu_supported {
        print_stages("GPU passes", &report.gpu, Some(wall));
    } else {
        eprintln!("(no TIMESTAMP_QUERY on this device: GPU table unavailable)");
    }
    print_stages("CPU stages", &report.cpu, None);
}

fn print_stages(title: &str, stages: &[Stage], wall: Option<f64>) {
    let total: Duration = stages.iter().map(|s| s.total).sum();
    match wall {
        Some(wall) => eprintln!(
            "{title}: {:.2} s busy ({:.0} % of wall)",
            total.as_secs_f64(),
            100.0 * total.as_secs_f64() / wall
        ),
        None => eprintln!("{title}:"),
    }
    eprintln!(
        "  {:<40} {:>7} {:>10} {:>9} {:>9}",
        "label", "count", "total ms", "mean ms", "max ms"
    );
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    for s in stages {
        eprintln!(
            "  {:<40} {:>7} {:>10.1} {:>9.2} {:>9.2}",
            s.label,
            s.count,
            ms(s.total),
            ms(s.total) / f64::from(s.count.max(1)),
            ms(s.max)
        );
    }
}

/// The folder given, or the frames' folder.
fn stack_name(inputs: &[PathBuf], frames: &[PathBuf]) -> String {
    let dir = match inputs {
        [one] if one.is_dir() => one.clone(),
        _ => frames[0].parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "stack".into())
}

/// A frame's size: from its header for a standard image, else one decode.
fn frame_size(path: &Path) -> Result<(u32, u32)> {
    if let Some(header) = deptheos_app::header_info(path, deptheos_app::CameraRaw::LibRaw) {
        return Ok((header.width, header.height));
    }
    let image = deptheos_app::load_image(path, deptheos_app::CameraRaw::LibRaw)?;
    Ok((image.width(), image.height()))
}

fn format_name(path: &Path) -> String {
    match (ExportFormat::of(path), path.extension().and_then(|e| e.to_str())) {
        (Some(format), _) => format.label().into(),
        (None, Some(e)) => e.to_ascii_uppercase(),
        (None, None) => "?".into(),
    }
}

impl Machine {
    fn default_label(&self) -> String {
        match self.gpu_cores {
            Some(cores) => format!("{} ({cores}-core GPU)", self.cpu),
            None if self.cpu == self.gpu => self.cpu.clone(),
            None => format!("{} + {}", self.cpu, self.gpu),
        }
    }
}

fn machine(ctx: &GpuContext) -> Machine {
    let info = &ctx.adapter_info;
    let (os, cpu, memory, gpu_cores) = platform();
    let (power, power_mode) = power();
    Machine {
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

/// Output of a command, trimmed; `None` when it cannot run or fails.
fn sh(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// OS name and version, CPU name, memory in bytes, and the GPU core count
/// where the platform reports one.
#[cfg(target_os = "macos")]
fn platform() -> (String, String, Option<u64>, Option<u32>) {
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
fn platform() -> (String, String, Option<u64>, Option<u32>) {
    let reg = |key: &str, value: &str| {
        sh("reg", &["query", key, "/v", value])
            .and_then(|s| s.split("REG_").nth(1).map(str::to_string))
            .and_then(|s| s.split_once(char::is_whitespace).map(|(_, v)| v.trim().to_string()))
    };
    // ProductName still says "Windows 10" on Windows 11; the build tells them apart.
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
    // Installed memory, as a Mac's hw.memsize reports it (the OS sees a little less).
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
fn platform() -> (String, String, Option<u64>, Option<u32>) {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "Linux".into());
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_else(|| "?".into());
    let memory = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        s.lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    });
    (os, cpu, memory, None)
}

/// The power source and the energy mode, from `pmset`.
#[cfg(target_os = "macos")]
fn power() -> (Option<String>, Option<&'static str>) {
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
            "0" => Some("automatic"),
            "1" => Some("low power"),
            "2" => Some("high power"),
            _ => None,
        }
    });
    (source, mode)
}

#[cfg(not(target_os = "macos"))]
fn power() -> (Option<String>, Option<&'static str>) {
    (None, None)
}

/// Today's local date as YYYY-MM-DD.
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_result_compares_sample_for_sample() {
        let saved: Vec<u8> = [0u16, 100, 65535].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(compare_samples(&[0, 100, 65535], &saved), "identical (3 samples)");
        let said = compare_samples(&[0, 103, 65535], &saved);
        assert!(
            said.starts_with("1 of 3 samples differ") && said.contains("max 3/65535"),
            "{said}"
        );
        assert_eq!(compare_samples(&[0, 100], &saved), "size differs (3 vs 2 samples)");
    }
}
