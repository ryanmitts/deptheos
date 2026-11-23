//! Export… from the Library, without leaving it, as on iOS: the render is
//! read on a worker, the Export sheet (`export.rs`) asks where, and it is
//! written with its paint as the window's one export (`exporting.rs`).

use std::sync::Arc;

use gpui::{Context, Window};

use super::AppView;
use super::export::ExportOf;
use super::exporting::Exporting;
use crate::notices::{Kind, Subject};
use crate::words::{Wording as _, say};
use crate::work;

impl AppView {
    /// Export… from the Library: read the render, then ask where to export
    /// it. Nothing starts while a run is under way or another export is.
    pub(crate) fn export_from_library(
        &mut self,
        id: deptheos_app::library::history::RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx)
            || self.is_exporting()
            || !matches!(self.sheet, super::Sheet::None)
        {
            return;
        }
        let renders = self.renders.read(cx);
        let Some(render) = renders.find(&id).cloned() else { return };
        let store = renders.store().clone();
        let words = cx.wording().clone();
        let failed = say!(cx, "library-export-read-failed");
        let task = cx.spawn_in(window, async move |this, cx| {
            let read = work::interactive("library-export-read", {
                let id = render.id.clone();
                move || {
                    let render_dir = store.render_path(&id);
                    let retouched =
                        deptheos_app::paint::store::has_paint(&render_dir).unwrap_or(false);
                    store
                        .load_render(&id)
                        .map(|stack| (stack, retouched))
                        .map_err(deptheos_app::Error::Store)
                }
            })
            .await;
            let read = crate::notices::worked(&words, &failed, read);
            this.update_in(cx, |this, window, cx| {
                this.set_exporting(Exporting::Idle, cx);
                match read {
                    // A run started meanwhile: nothing is asked over it.
                    Ok(_) if this.is_processing(cx) => {}
                    Ok((stack, retouched)) => {
                        let (render, stack) = (Box::new(render), Arc::new(stack));
                        let of = ExportOf::Render { render, stack, retouched };
                        this.open_export_sheet(of, window, cx);
                    }
                    Err(why) => {
                        this.post(Kind::Error, Subject::Export, why, Vec::new(), window, cx)
                    }
                }
            })
            .ok();
        });
        self.set_exporting(Exporting::Reading { render: id, _task: task }, cx);
    }
}
