//! Help ▸ Benchmark… dialog: times the live Pyramid + ECC stacking path on
//! the currently open stack or a bundled sample, and lets the user copy a
//! Markdown summary or save the JSON record for `web/measured/benchmarks/`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deptheos_app::runs::bench::{self, BenchConfig, BenchProgress, BenchReport, MachineInfo};
use deptheos_app::{Cancel, Engine};
use gpui::prelude::*;
use gpui::{App, ClipboardItem, Context, EventEmitter, SharedString, Task, Window, div};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use crate::app::ModalDialog;
use crate::form::{heading, note, radio_row};

/// One selectable stack to benchmark.
#[derive(Clone, Debug)]
pub struct BenchSource {
    pub label: String,
    pub stack_name: String,
    pub paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchMode {
    Quick,
    Thorough,
}

impl BenchMode {
    const ALL: [Self; 2] = [Self::Quick, Self::Thorough];

    fn config(self) -> BenchConfig {
        match self {
            Self::Quick => BenchConfig::quick(),
            Self::Thorough => BenchConfig::thorough(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Quick => "Quick (3 runs · 5 s pause)",
            Self::Thorough => "Thorough (5 runs · 30 s cooldown)",
        }
    }
}

pub enum BenchDialogEvent {
    SaveJson { filename: String, json: String },
    Close,
}

enum State {
    Idle,
    Running { cancel: Cancel, progress: BenchProgress, _task: Task<()> },
    Done(Box<BenchReport>),
    Failed(String),
}

pub struct BenchDialog {
    engine: Arc<Engine>,
    sources: Vec<BenchSource>,
    selected_source: usize,
    mode: BenchMode,
    state: State,
    copied: bool,
}

impl EventEmitter<BenchDialogEvent> for BenchDialog {}

impl BenchDialog {
    pub fn new(engine: Arc<Engine>, sources: Vec<BenchSource>) -> Self {
        Self {
            engine,
            sources,
            selected_source: 0,
            mode: BenchMode::Quick,
            state: State::Idle,
            copied: false,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.sources.get(self.selected_source).cloned() else { return };
        if source.paths.len() < 2 {
            self.state = State::Failed("Need at least 2 frames to run a benchmark.".into());
            cx.notify();
            return;
        }
        let cancel = Cancel::root();
        let worker_cancel = cancel.clone();
        let engine = self.engine.clone();
        let config = self.mode.config();
        let shared_progress: Arc<Mutex<BenchProgress>> =
            Arc::new(Mutex::new(BenchProgress::Warmup));
        let worker_progress = shared_progress.clone();
        let (tx, rx) = futures::channel::oneshot::channel();

        let _ = std::thread::Builder::new().name("deptheos-bench".into()).spawn(move || {
            let machine = MachineInfo::detect(engine.gpu());
            let mut on_progress = |p: BenchProgress| {
                if let Ok(mut guard) = worker_progress.lock() {
                    *guard = p;
                }
            };
            let result = bench::run_bench(
                &engine,
                source.stack_name,
                &source.paths,
                config,
                machine,
                None,
                &worker_cancel,
                &mut on_progress,
            );
            let _ = tx.send(result);
        });

        let task = cx.spawn(async move |this, cx| {
            let mut rx = rx;
            loop {
                if let Ok(Some(result)) = rx.try_recv() {
                    let _ = this.update(cx, |this, cx| {
                        this.copied = false;
                        this.state = match result {
                            Ok(report) => State::Done(Box::new(report)),
                            Err(deptheos_app::Error::Core(deptheos_app::CoreError::Canceled)) => {
                                State::Idle
                            }
                            Err(err) => State::Failed(err.to_string()),
                        };
                        cx.notify();
                    });
                    break;
                }
                let latest = shared_progress.lock().ok().map(|g| g.clone());
                if let Some(p) = latest {
                    let alive = this.update(cx, |this, cx| {
                        if let State::Running { progress, .. } = &mut this.state
                            && *progress != p
                        {
                            *progress = p;
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
        });

        self.copied = false;
        self.state = State::Running { cancel, progress: BenchProgress::Warmup, _task: task };
        cx.notify();
    }

    fn cancel_run(&mut self, cx: &mut Context<Self>) {
        if let State::Running { cancel, .. } = &self.state {
            cancel.cancel();
        }
        self.state = State::Idle;
        cx.notify();
    }

    fn progress_line(progress: &BenchProgress) -> String {
        match progress {
            BenchProgress::Warmup => "Warming up GPU shaders…".into(),
            BenchProgress::Cooldown { round, total_rounds, seconds_left } => {
                format!("Cooling down before run {round} of {total_rounds} ({seconds_left} s)…")
            }
            BenchProgress::Running { round, total_rounds, frame, frames } => {
                format!("Run {round} of {total_rounds} · Frame {frame} of {frames}")
            }
        }
    }

    fn json_filename(report: &BenchReport) -> String {
        let slug = |s: &str| {
            s.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_string()
        };
        format!(
            "{}-{}-{}.json",
            slug(&report.label),
            slug(&report.stack.name),
            report.stack.format.to_ascii_lowercase()
        )
    }
}

impl Drop for BenchDialog {
    fn drop(&mut self) {
        if let State::Running { cancel, .. } = &self.state {
            cancel.cancel();
        }
    }
}

impl Render for BenchDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let running = self.is_running();
        const SOURCE_SLOTS: [usize; 4] = [0, 1, 2, 3];
        let indices = &SOURCE_SLOTS[..self.sources.len().min(SOURCE_SLOTS.len())];
        let source_labels: Vec<String> = self.sources.iter().map(|s| s.label.clone()).collect();

        let body = match &self.state {
            State::Idle => div().into_any_element(),
            State::Running { progress, .. } => v_flex()
                .gap_1()
                .p_3()
                .rounded_md()
                .bg(cx.theme().secondary)
                .child(div().text_sm().child(Self::progress_line(progress)))
                .into_any_element(),
            State::Failed(msg) => div()
                .text_sm()
                .text_color(cx.theme().danger_foreground)
                .child(msg.clone())
                .into_any_element(),
            State::Done(report) => {
                let r = &report.run;
                let s = &report.stack;
                let copied = self.copied;
                let md = report.to_markdown();
                let json = report.to_json();
                let filename = Self::json_filename(report);
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child(format!(
                        "{:.2} s median · {:.0} MP/s · {:.1} ms/frame",
                        r.seconds, r.mpx_per_s, r.ms_per_frame
                    )))
                    .child(note(
                        format!(
                            "Range {:.2}–{:.2} s · {} × {} × {} {} ({:.1} MP) · {}",
                            r.fastest,
                            r.slowest,
                            s.frames,
                            s.width,
                            s.height,
                            s.format,
                            s.megapixels,
                            report.label
                        ),
                        muted,
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_1()
                            .child(
                                Button::new("bench-copy")
                                    .small()
                                    .label(if copied { "Copied" } else { "Copy Summary" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            md.clone(),
                                        ));
                                        this.copied = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("bench-save-json")
                                    .small()
                                    .label("Save JSON…")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(BenchDialogEvent::SaveJson {
                                            filename: filename.clone(),
                                            json: json.clone(),
                                        });
                                    })),
                            ),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(note(
                "Times Pyramid + ECC end to end (disk read, decode, align, stack, and viewer image).",
                muted,
            ))
            .when(!self.sources.is_empty(), |el| {
                el.child(heading("Stack")).child(radio_row(
                    RadioGroup::vertical("bench-source"),
                    indices,
                    move |i| source_labels[i].clone(),
                    self.selected_source,
                    move |_| !running,
                    cx.listener(|this, &idx: &usize, _, cx| {
                        this.selected_source = idx;
                        cx.notify();
                    }),
                ))
            })
            .when(self.sources.is_empty(), |el| {
                el.child(note(
                    "Open a stack with at least 2 frames to run a benchmark.",
                    muted,
                ))
            })
            .child(heading("Mode"))
            .child(radio_row(
                RadioGroup::vertical("bench-mode"),
                &BenchMode::ALL,
                |m| m.label().to_string(),
                self.mode,
                move |_| !running,
                cx.listener(|this, mode: &BenchMode, _, cx| {
                    this.mode = *mode;
                    cx.notify();
                }),
            ))
            .child(body)
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("bench-close")
                            .label(if running { "Cancel" } else { "Close" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.is_running() {
                                    this.cancel_run(cx);
                                } else {
                                    cx.emit(BenchDialogEvent::Close);
                                }
                            })),
                    )
                    .child(
                        Button::new("bench-run")
                            .label(if running { "Running…" } else { "Run Benchmark" })
                            .primary()
                            .disabled(running || self.sources.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                    ),
            )
    }
}

impl ModalDialog for BenchDialog {
    const WIDTH: f32 = 480.0;

    fn title(_cx: &App) -> SharedString {
        "Benchmark".into()
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if !self.is_running() && !self.sources.is_empty() {
            self.start(cx);
        }
    }
}
