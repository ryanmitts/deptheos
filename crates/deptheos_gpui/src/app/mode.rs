//! What is being done to the render shown: viewing it, Retouch or Adjust,
//! one at a time. The viewer's place and the two engine sessions follow
//! [`AppView::set_mode`].

use std::sync::Arc;

use deptheos_app::CompletedStack;
use deptheos_app::library::history::RenderId;
use gpui::{Context, Entity, SharedString, Window};

use super::AppView;
use crate::adjust::{Adjust, AdjustEvent};
use crate::notices::{Kind, Subject};
use crate::paint::{Painting, SourcesTab};
use crate::viewer::{Tool, ViewEvent};

/// What is being done to the render shown.
///
/// Lifecycle: `Viewing`, `Retouch` (paint's [`Painting`]) or `Adjust`
/// ([`Adjusting`]), held by `AppView`, changed only by
/// [`AppView::set_mode`], which closes the one left before opening the
/// next, so the two can't both be open.
///
/// - `Viewing` → `Retouch`, `Adjust`: opened on the saved render shown.
/// - `Retouch` → `Retouch`, `Adjust` → `Adjust`: another render shown.
/// - `Retouch` → `Adjust`, `Adjust` → `Retouch`: the one left closed
///   first, as leaving it for `Viewing` does.
/// - `Retouch`, `Adjust` → `Viewing`: left, or the render deleted.
pub(crate) enum Mode {
    Viewing,
    Retouch(Painting),
    Adjust(Adjusting),
}

/// The render adjusted.
pub(crate) struct Adjusting {
    pub render: RenderId,
    pub stack: Arc<CompletedStack>,
}

impl Mode {
    fn name(&self) -> &'static str {
        match self {
            Mode::Viewing => "viewing",
            Mode::Retouch(_) => "retouch",
            Mode::Adjust(_) => "adjust",
        }
    }
}

impl AppView {
    /// Change what is being done to the render shown, closing what was open
    /// first; returns what it was.
    pub(super) fn set_mode(&mut self, next: Mode, cx: &mut Context<Self>) -> Mode {
        let before = std::mem::replace(&mut self.mode, Mode::Viewing);
        tracing::debug!(from = before.name(), to = next.name(), "mode");
        let same_kind = std::mem::discriminant(&before) == std::mem::discriminant(&next);
        self.leave_mode(&before, same_kind, cx);
        self.enter_mode(&next, cx);
        self.mode = next;
        self.sync_on_screen(cx);
        cx.notify();
        before
    }

    /// Close `before`; `switching` keeps its session open for another render.
    fn leave_mode(&mut self, before: &Mode, switching: bool, cx: &mut Context<Self>) {
        match before {
            Mode::Viewing => {}
            Mode::Retouch(painting) => {
                if !switching {
                    let left = self.viewer.read(cx).retouched();
                    self.paint.update(cx, |p, cx| {
                        if let Some(left) = left {
                            p.remember(left);
                        }
                        p.close(cx);
                    });
                }
                self.painted_thumbnail(painting, cx);
                if !switching {
                    self.viewer.update(cx, |v, cx| {
                        v.release_keys(cx);
                        v.set_tool(Tool::Pan, cx);
                        v.on(ViewEvent::Retouch(None), cx);
                        v.clear_compare(cx);
                    });
                }
            }
            Mode::Adjust(_) => {
                if !switching {
                    self.adjust.update(cx, |a, cx| a.leave(cx));
                    self.viewer.update(cx, |v, cx| {
                        v.release_keys(cx);
                        v.set_tool(Tool::Pan, cx);
                        v.on(ViewEvent::Adjust(false), cx);
                    });
                }
            }
        }
    }

    fn enter_mode(&mut self, next: &Mode, cx: &mut Context<Self>) {
        match next {
            Mode::Viewing => {}
            Mode::Retouch(painting) => {
                self.renders.update(cx, |r, cx| r.clear_compare(cx));
                // Retouch shows what it was left showing (ADR-149).
                let left = self.paint.read(cx).retouched();
                self.viewer.update(cx, |v, cx| {
                    v.cancel_loads(cx);
                    v.set_tool(Tool::Paint, cx);
                    v.on(ViewEvent::Retouch(Some(left)), cx);
                });
                self.paint.update(cx, |p, cx| p.open(painting, cx));
            }
            Mode::Adjust(adjusting) => {
                self.renders.update(cx, |r, cx| r.clear_compare(cx));
                self.viewer.update(cx, |v, cx| {
                    v.cancel_loads(cx);
                    v.set_tool(Tool::Pan, cx);
                    v.on(ViewEvent::Adjust(true), cx);
                });
                let (render, stack) = (adjusting.render.clone(), adjusting.stack.clone());
                // Before shows the render with its paint, as stored.
                let flattened =
                    self.paint.update(cx, |p, _| p.flatten(render.clone(), stack.clone()));
                self.adjust.update(cx, |a, cx| a.open(render, stack, flattened, cx));
            }
        }
    }

    /// Retouch left `painting`'s render: if it was painted, its thumbnail is
    /// drawn again with its paint, flattened after the session's last commit.
    fn painted_thumbnail(&mut self, painting: &Painting, cx: &mut Context<Self>) {
        if self.paint.read(cx).generation(&painting.render) == painting.opened {
            return;
        }
        let (render, stack) = (painting.render.clone(), painting.stack.clone());
        let flattened = self.paint.update(cx, |p, _| p.flatten(render.clone(), stack.clone()));
        self.renders.update(cx, |r, cx| r.rewrite_thumbnail(render, stack, Some(flattened), cx));
    }

    /// Retouch is open.
    pub(crate) fn is_retouching(&self) -> bool {
        matches!(self.mode, Mode::Retouch(_))
    }

    /// The render painted, while Retouch is open.
    pub(crate) fn painting(&self) -> Option<&Painting> {
        match &self.mode {
            Mode::Retouch(painting) => Some(painting),
            Mode::Viewing | Mode::Adjust(_) => None,
        }
    }

    /// The render adjusted, while Adjust is open.
    pub(crate) fn adjusting(&self) -> Option<&Adjusting> {
        match &self.mode {
            Mode::Adjust(adjusting) => Some(adjusting),
            Mode::Viewing | Mode::Retouch(_) => None,
        }
    }

    /// What Retouch's left panel lists, while Retouch is open.
    pub(crate) fn sources_tab(&self) -> Option<SourcesTab> {
        self.painting().map(|p| p.sources)
    }

    /// Retouch's left panel switch: list the frames or the renders.
    pub(crate) fn set_sources_tab(&mut self, tab: SourcesTab, cx: &mut Context<Self>) {
        if let Mode::Retouch(painting) = &mut self.mode
            && painting.sources != tab
        {
            tracing::debug!("retouch: sources {:?} -> {tab:?}", painting.sources);
            painting.sources = tab;
            cx.notify();
        }
    }

    /// Paint from render `render`, named `label` in the source pane.
    pub(crate) fn paint_from_render(
        &mut self,
        render: &RenderId,
        label: SharedString,
        cx: &mut Context<Self>,
    ) {
        let Mode::Retouch(painting) = &mut self.mode else { return };
        let kind = deptheos_app::paint::SourceKind::Render { render: render.as_str().to_string() };
        painting.label(kind, label);
        self.paint.update(cx, |p, cx| p.pick_render(render, cx));
    }

    /// The source painted from is ready: shown in the source pane, named
    /// from Retouch's picks.
    pub(super) fn show_source(
        &mut self,
        key: &RenderId,
        image: crate::images::TiledImage,
        cx: &mut Context<Self>,
    ) {
        let Some(painting) = self.painting().filter(|p| p.render == *key) else { return };
        let labels = self.paint.read(cx).source_labels(painting, cx);
        self.viewer.update(cx, |v, cx| v.show_source(image, labels, cx));
    }

    /// Adjust the saved render shown, or leave Adjust.
    pub(super) fn toggle_adjust(&mut self, cx: &mut Context<Self>) {
        if self.adjusting().is_some() {
            self.set_mode(Mode::Viewing, cx);
        } else {
            self.adjust_shown(cx);
        }
    }

    /// Adjust the saved render shown, as Retouch opens on it.
    pub(super) fn adjust_shown(&mut self, cx: &mut Context<Self>) {
        if !self.can_paint(cx) {
            return;
        }
        let Some((stack, render)) = self.result_doc(cx) else { return };
        self.set_mode(Mode::Adjust(Adjusting { render, stack }), cx);
    }

    /// What Adjust says besides its panel and pictures.
    pub(super) fn on_adjust(
        &mut self,
        _: &Entity<Adjust>,
        event: &AdjustEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AdjustEvent::Rewritten(id) => {
                self.renders.update(cx, |r, cx| r.thumbnail_rewritten(id, cx))
            }
            AdjustEvent::Notice(text) => {
                self.post(Kind::Info, Subject::Alone, text.clone(), Vec::new(), window, cx)
            }
            AdjustEvent::Drawn { key, picture } => {
                let shown = self.result_doc(cx).is_some_and(|(_, k)| k == *key);
                if shown
                    && self.adjusting().is_none()
                    && !self.is_retouching()
                    && let Some(picture) = picture.take()
                {
                    self.viewer.update(cx, |v, cx| v.show_drawn(picture, cx));
                }
            }
            AdjustEvent::Closed { key, redraw } => {
                // The session left the render on its own (a delete): so
                // does the window.
                if self.adjusting().is_some_and(|a| a.render == *key) {
                    self.set_mode(Mode::Viewing, cx);
                }
                // The screen doesn't show the value saved: open it again.
                let shown = self.result_doc(cx).is_some_and(|(_, k)| k == *key);
                if *redraw && shown && self.adjusting().is_none() {
                    tracing::debug!("adjust: the render drawn again under its saved value");
                    let key = key.clone();
                    self.renders.update(cx, |r, cx| r.open(&key, window, cx));
                }
            }
        }
    }
}
