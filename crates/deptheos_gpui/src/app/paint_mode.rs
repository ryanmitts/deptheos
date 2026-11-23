//! Paint mode: when the result can be retouched, and going in and out.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::CompletedStack;
use deptheos_app::library::history::RenderId;
use gpui::{App, Context};

use super::AppView;
use crate::paint::{OpenRender, Paint};
use crate::viewer::{Tool, ViewEvent};
use crate::words::say;

impl AppView {
    /// The result on screen and the render it was saved as, once it was:
    /// what paint is keyed by.
    pub(super) fn result_doc(&self, cx: &App) -> Option<(Arc<CompletedStack>, RenderId)> {
        self.stacking.read(cx).saved_result()
    }

    pub(crate) fn can_paint(&self, cx: &App) -> bool {
        self.paint_blocked(cx).is_none()
    }

    /// Why paint mode can't open; `None` when it can. The result must be
    /// saved as a render first, so paint is saved with it from the first stroke.
    pub(crate) fn paint_blocked(&self, cx: &App) -> Option<String> {
        if !self.has_result(cx) {
            return Some(say!(cx, "paint-blocked-no-result"));
        }
        if let Some(busy) = self.busy_reason(cx) {
            return Some(busy);
        }
        if self.result_doc(cx).is_some() {
            return None;
        }
        if self.renders.read(cx).is_saving() {
            Some(say!(cx, "paint-blocked-saving"))
        } else {
            Some(say!(cx, "paint-blocked-not-saved"))
        }
    }

    pub(super) fn toggle_paint(&mut self, cx: &mut Context<Self>) {
        if self.paint.read(cx).is_open() {
            self.leave_paint(cx);
        } else {
            self.enter_paint(cx);
        }
    }

    /// Paint the result on screen: the canvas in the result's pane, the
    /// source beside it, the Paint tool on.
    pub(crate) fn enter_paint(&mut self, cx: &mut Context<Self>) {
        if !self.can_paint(cx) {
            return;
        }
        // Only a saved render is painted (see `paint_blocked`).
        let Some((stack, render)) = self.result_doc(cx) else { return };
        let files = self.files.read(cx);
        // Stepping goes through the list, plus any of the render's own
        // frames outside it.
        let mut frames: Vec<PathBuf> = files.rows().map(|(_, path)| path).collect();
        let outside: Vec<PathBuf> =
            stack.paths.iter().filter(|p| !frames.contains(p)).cloned().collect();
        frames.extend(outside);
        // The frame selected, if the render stacked it; else the session
        // picks the first it did.
        let first = files.selected(cx).and_then(|id| files.path(id).map(Path::to_path_buf));
        self.renders.update(cx, |r, cx| r.clear_compare(cx));
        // Retouch shows what it was left showing (ADR-149).
        let left = self.paint.read(cx).retouched();
        self.viewer.update(cx, |v, cx| {
            v.cancel_loads(cx);
            v.set_tool(Tool::Paint, cx);
            v.on(ViewEvent::Retouch(Some(left)), cx);
        });
        let render = OpenRender { stack, render, frames, first_source: first };
        self.paint.update(cx, |p, cx| p.open(render, cx));
        // Revert to #n is off in a tile's menu while Retouch is open.
        self.sync_on_screen(cx);
        cx.notify();
    }

    /// Back to stack mode, keeping the painted canvas on screen.
    pub(crate) fn leave_paint(&mut self, cx: &mut Context<Self>) {
        if !self.paint.read(cx).is_open() {
            return;
        }
        let left = self.viewer.read(cx).retouched();
        self.paint.update(cx, |p, cx| {
            if let Some(left) = left {
                p.remember(left);
            }
            p.close(cx);
        });
        self.viewer.update(cx, |v, cx| {
            v.release_keys(cx);
            v.set_tool(Tool::Pan, cx);
            v.on(ViewEvent::Retouch(None), cx);
            v.clear_compare(cx);
        });
        self.sync_on_screen(cx);
        cx.notify();
    }

    pub(super) fn with_paint(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Paint, &mut Context<Paint>),
    ) {
        if self.paint.read(cx).is_open() {
            self.paint.update(cx, f);
        }
    }
}
