//! The window's one export: the result on screen's (`export.rs`) or a
//! Library render's, read first (`library_export.rs`). While one is read or
//! written, the other Export… items are disabled or inert and both bars say
//! what it does.

use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::RenderId;
use deptheos_app::paint::Flattened;
use deptheos_app::{CompletedStack, SaveOptions};
use gpui::{App, Context, Task, Window};

use super::AppView;
use crate::words::say;

/// The window's one export, the result on screen's or a Library render's.
///
/// Lifecycle: `Idle`, `Reading` a render for the sheet, or `Writing`, held
/// by `AppView` and changed only by [`AppView::set_exporting`]. Each
/// worker's task lives in its state, so a result lands only while current.
///
/// ```text
/// Idle ──the Library's Export…──▶ Reading ──read──▶ Idle, the Export sheet open
///                                    └──failed────▶ Idle, said in a notice
/// Idle ──the sheet's Export, or Export Again──▶ Writing ──written or failed──▶ Idle, a notice
/// ```
pub(crate) enum Exporting {
    Idle,
    /// Reading `render` from the Library for the sheet.
    Reading {
        render: RenderId,
        _task: Task<()>,
    },
    /// Writing to `path`.
    Writing {
        path: PathBuf,
        _task: Task<()>,
    },
}

impl Exporting {
    fn name(&self) -> &'static str {
        match self {
            Exporting::Idle => "idle",
            Exporting::Reading { .. } => "reading",
            Exporting::Writing { .. } => "writing",
        }
    }
}

/// What an export writes.
pub(super) enum Written {
    /// The result on screen, with its paint.
    Shown,
    /// A Library render read for the sheet, with its paint.
    Render(RenderId, Arc<CompletedStack>),
}

impl AppView {
    pub(super) fn set_exporting(&mut self, next: Exporting, cx: &mut Context<Self>) {
        tracing::debug!("export: {} -> {}", self.exporting.name(), next.name());
        self.exporting = next;
        cx.notify();
    }

    /// An export is being read or written: no other starts.
    pub(crate) fn is_exporting(&self) -> bool {
        !matches!(self.exporting, Exporting::Idle)
    }

    /// An export's file is being written, which quit waits for.
    pub(super) fn is_writing_export(&self) -> bool {
        matches!(self.exporting, Exporting::Writing { .. })
    }

    /// Write `what` to `path` with `options` and any paint, reporting in a
    /// notice (a failure offers to retry). Nothing during a stack or export.
    pub(super) fn write_to(
        &mut self,
        what: Written,
        path: PathBuf,
        options: SaveOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_exporting() {
            return;
        }
        let (stack, painted, saved, again) = match what {
            Written::Shown => {
                let stacking = self.stacking.read(cx);
                if stacking.is_running() {
                    return;
                }
                let Some(stack) = stacking.result().cloned() else { return };
                let saved = self.result_doc(cx);
                let painted = saved
                    .clone()
                    .map(|(stack, key)| self.paint.update(cx, |p, _| p.flatten(key, stack)));
                (stack, painted, saved.map(|(_, key)| key), None)
            }
            Written::Render(id, stack) => {
                let painted = self.paint.update(cx, |p, _| p.flatten(id.clone(), stack.clone()));
                (stack, Some(painted), Some(id.clone()), Some(id))
            }
        };
        let engine = self.sequences.read(cx).engine().clone();
        let saved = saved.map(|key| (self.renders.read(cx).store().clone(), key));
        let task = cx.spawn_in(window, {
            let path = path.clone();
            async move |this, cx| {
                let target = path.clone();
                let saved = save_stack(engine, stack, painted, saved, target, options).await;
                this.update_in(cx, |this, window, cx| {
                    this.set_exporting(Exporting::Idle, cx);
                    match saved {
                        Ok(()) => this.exported(&path, window, cx),
                        Err(e) => {
                            let error = crate::notices::explain(&e, crate::words::of(cx));
                            // Export… again, to another folder or format.
                            let again = this.action(
                                say!(cx, "app-stacked-export"),
                                move |this, window, cx| match &again {
                                    None => this.save_result(window, cx),
                                    Some(id) => this.export_from_library(id.clone(), window, cx),
                                },
                                cx,
                            );
                            this.export_failed(&path, &error, again, window, cx);
                        }
                    }
                })
                .ok();
            }
        });
        self.set_exporting(Exporting::Writing { path, _task: task }, cx);
    }

    /// What the export is doing, for the bars: "Reading #3…", "Exporting
    /// bee-4.tif…".
    pub(crate) fn export_progress_text(&self, cx: &App) -> Option<String> {
        match &self.exporting {
            Exporting::Idle => None,
            Exporting::Reading { render, .. } => {
                let number = self.renders.read(cx).find(render)?.number;
                Some(say!(cx, "library-export-reading", number = number.to_string()))
            }
            Exporting::Writing { path, .. } => {
                Some(say!(cx, "export-exporting", name = file_label(path)))
            }
        }
    }
}

/// Save `stack` to `path` with `options` on a worker, with the pixels
/// `painted` sends in place of its own if it sends any (it retouched), under
/// the adjustments `saved` names in the Library, read after its pending
/// writes.
async fn save_stack(
    engine: Arc<deptheos_app::Engine>,
    stack: Arc<CompletedStack>,
    painted: Option<std::sync::mpsc::Receiver<Flattened>>,
    saved: Option<(Arc<deptheos_app::library::history::History>, RenderId)>,
    path: PathBuf,
    options: SaveOptions,
) -> Result<(), deptheos_app::Error> {
    crate::work::write("export", move || {
        let adjusted = match saved {
            Some((history, key)) => history.adjustments_to_export(&key)?,
            None => deptheos_app::library::adjustments::RenderAdjustments::NONE,
        };
        engine.save_flattened(deptheos_app::Look::saved(&stack, &adjusted), painted, &path, options)
    })
    .await
    .unwrap_or_else(|e| Err(deptheos_app::Error::InvalidState(format!("save {e}"))))
}
