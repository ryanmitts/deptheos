//! The sheet over the window: one dialog at a time, as a [`Sheet`].

use gpui::prelude::*;
use gpui::{App, Context, Entity, Subscription, Task, Window, px};
use gpui_kit::component::WindowExt as _;

use super::AppView;
use crate::batch_dialog::BatchDialog;

/// A sheet's dialog and its event subscription.
pub(crate) struct Modal<V> {
    pub(crate) dialog: Entity<V>,
    _events: Subscription,
}

/// The Export sheet, and the free-name lookup its Export started. The lookup
/// lives only here: closing drops (cancels) it; its dialog shows Export busy
/// while it runs.
pub(crate) struct ExportSheet {
    pub(crate) modal: Modal<crate::export_dialog::ExportDialog>,
    pub(super) finding: Option<Task<()>>,
}

/// The sheet over the window, one at a time: a sheet asked while another is
/// up isn't opened.
///
/// Lifecycle: `None`, `Batch` (Batch…), `Grow` (Export Grow Sequence…) or
/// `Export` (Export…), held by `AppView` and changed only by
/// [`AppView::set_sheet`]. `None` → any through [`AppView::open_modal`];
/// each → `None` when its event says done, when closed, or (Export) once its
/// name is found, before the file is written.
pub(crate) enum Sheet {
    None,
    Batch(Modal<BatchDialog>),
    Bench(Modal<crate::bench_dialog::BenchDialog>),
    Grow(Modal<crate::grow_dialog::GrowDialog>),
    Export(ExportSheet),
}

impl Sheet {
    fn name(&self) -> &'static str {
        match self {
            Sheet::None => "none",
            Sheet::Batch(_) => "batch",
            Sheet::Bench(_) => "bench",
            Sheet::Grow(_) => "grow",
            Sheet::Export(_) => "export",
        }
    }

    /// The dialog's entity, while one is up.
    fn dialog(&self) -> Option<gpui::EntityId> {
        match self {
            Sheet::None => None,
            Sheet::Batch(m) => Some(m.dialog.entity_id()),
            Sheet::Bench(m) => Some(m.dialog.entity_id()),
            Sheet::Grow(m) => Some(m.dialog.entity_id()),
            Sheet::Export(s) => Some(s.modal.dialog.entity_id()),
        }
    }
}

impl AppView {
    /// Show `dialog` as the sheet, held as `sheet` makes it, unless one is up.
    /// Its first event closes it, then `on_event` runs; otherwise it just
    /// leaves. Return presses its primary button ([`ModalDialog`]), which the
    /// kit's handler would instead close unfinished.
    pub(crate) fn open_modal<V: Render + gpui::EventEmitter<E> + ModalDialog, E: 'static>(
        &mut self,
        dialog: Entity<V>,
        sheet: fn(Modal<V>) -> Sheet,
        on_event: impl Fn(&mut Self, &E, &mut Window, &mut Context<Self>) -> bool + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.sheet, Sheet::None) {
            tracing::debug!("sheet: {} up, another not opened over it", self.sheet.name());
            return;
        }
        // `on_event` says whether it is done; a request that can't go ahead
        // keeps it open as chosen.
        let events = cx.subscribe_in(&dialog, window, move |this, _, event: &E, window, cx| {
            if on_event(this, event, window, cx) {
                this.set_sheet(Sheet::None);
                window.close_dialog(cx);
            }
        });
        self.set_sheet(sheet(Modal { dialog: dialog.clone(), _events: events }));
        let this = cx.weak_entity();
        let title = V::title(cx);
        window.open_dialog(cx, move |d, _, _| {
            let this = this.clone();
            let confirmed = dialog.clone();
            let id = dialog.entity_id();
            let d = if title.is_empty() { d } else { d.title(title.clone()) };
            d.w(px(V::WIDTH))
                .child(dialog.clone())
                .on_ok(move |_, _, cx| {
                    confirmed.update(cx, |dialog, cx| dialog.confirm(cx));
                    // Its event closes it, if the request can go ahead.
                    false
                })
                .on_close(move |_, _, cx| {
                    this.update(cx, |this, _| {
                        // Already gone if its event closed it.
                        if this.sheet.dialog() == Some(id) {
                            this.set_sheet(Sheet::None);
                        }
                    })
                    .ok();
                })
        });
    }

    /// The only place [`Self::sheet`] changes.
    pub(crate) fn set_sheet(&mut self, sheet: Sheet) {
        tracing::debug!("sheet: {} -> {}", self.sheet.name(), sheet.name());
        self.sheet = sheet;
    }
}

/// A sheet's dialog: width, title (none when empty), and the primary button
/// Return presses too, emitting the event when the choice can go ahead and
/// doing nothing when disabled.
pub(crate) trait ModalDialog: Sized {
    const WIDTH: f32;
    fn title(_cx: &App) -> gpui::SharedString {
        gpui::SharedString::default()
    }
    fn confirm(&mut self, cx: &mut Context<Self>);
}
