//! Where the window goes: the Library or Stacking, a stack or render opened
//! from the Library, a failed batch stack retried, stack switches (keeping
//! the working version left) and a render's result on screen.

use std::sync::Arc;

use deptheos_app::PresentedStack;
use deptheos_app::library::history::{Failure, Render, RenderId};
use deptheos_app::library::stacks::Working;
use gpui::{Context, Window};

use super::{AppView, Screen};
use crate::notices::{Kind, Subject};
use crate::renders::OpenedRender;
use crate::viewer::{Look, ViewEvent};
use crate::words::{Wording as _, say};

impl AppView {
    /// Load a failed batch stack's images and settings to fix and restack;
    /// its stack goes on screen, keeping the one left.
    pub(crate) fn retry(&mut self, failure: &Failure, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_processing(cx) {
            return;
        }
        let fallback = Some(Working::of_failure(failure));
        self.switch_to(&failure.stack, fallback, window, cx, |this, incoming, window, cx| {
            let Some(incoming) = incoming else { return };
            this.forget_result(window, cx);
            this.edit(incoming, window, cx);
        });
    }

    /// Show Stacking or the Library. The Library rereads the stacks
    /// (switches write working versions) and selects the stack on screen if
    /// nothing is selected.
    pub(crate) fn set_screen(
        &mut self,
        screen: Screen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.screen == screen {
            return;
        }
        // The Library (⌥⌘2) is reached from Retouch too: it is done first.
        self.leave_paint(cx);
        tracing::debug!("screen: {:?} -> {screen:?}", self.screen);
        self.screen = screen;
        // Space's preview and Recently Deleted's list are the Library's:
        // they close with it.
        self.library.update(cx, |l, cx| {
            l.close_preview(cx);
            l.hide_deleted(cx);
        });
        if screen == Screen::Library {
            self.renders.update(cx, |r, cx| {
                r.refresh_stacks(cx);
                r.set_info(None, cx);
            });
            self.sync_library(cx);
            // Where you were: ↩ goes back to it.
            let open = self.stack.clone();
            self.library.update(cx, |l, cx| l.select_open(open, cx));
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Open `id` from the Library in Stacking: a failed stack to retry, a
    /// render in its stack showing it, with the working version as left.
    pub(crate) fn open_from_library(
        &mut self,
        id: RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return;
        }
        self.set_screen(Screen::Stacking, window, cx);
        if self.renders.read(cx).failure(&id).is_some() {
            self.renders.update(cx, |r, cx| r.retry(&id, cx));
            return;
        }
        if self.renders.read(cx).find(&id).is_none() {
            return;
        }
        if self.renders.read(cx).active(cx) != Some(&id) {
            self.renders.update(cx, |r, cx| r.open(&id, window, cx));
        } else {
            // Already on screen: it stays, not an earlier open still loading.
            self.renders.update(cx, |r, cx| {
                r.cancel_open();
                cx.notify();
            });
        }
    }

    /// Open a stack from the Library (Open, ↩): its working version as left,
    /// showing its base (or a frame without one).
    pub(crate) fn open_stack_from_library(
        &mut self,
        stack: &deptheos_app::library::history::StackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return;
        }
        self.set_screen(Screen::Stacking, window, cx);
        self.open_stack(stack, window, cx);
    }

    /// Put `stack`'s working version on screen with its base's result (or a
    /// frame); the stack already on screen only shows its base again.
    fn open_stack(
        &mut self,
        stack: &deptheos_app::library::history::StackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if *stack == self.stack {
            if let Some(base) = self.base.clone()
                && self.renders.read(cx).find(&base).is_some()
                && self.renders.read(cx).active(cx) != Some(&base)
            {
                self.renders.update(cx, |r, cx| r.open(&base, window, cx));
            }
            return;
        }
        self.switch_to(stack, None, window, cx, |this, incoming, window, cx| {
            let Some(incoming) = incoming else { return };
            this.forget_result(window, cx);
            let base = incoming.base.clone().filter(|b| this.renders.read(cx).find(b).is_some());
            this.edit(incoming, window, cx);
            match base {
                Some(base) => this.renders.update(cx, |r, cx| r.open(&base, window, cx)),
                None => this.show_first_included(window, cx),
            }
        });
    }

    /// Show an opened render's result, which save, compare and paint work
    /// from. Another stack's render switches to that stack first; what is
    /// shown never changes the working version.
    pub(super) fn restore(
        &mut self,
        opened: OpenedRender,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let OpenedRender { render, presented, .. } = opened;
        if render.stack == self.stack {
            self.show_render_result(&render, presented, window, cx);
            return;
        }
        if self.list_save.is_switching() {
            // Another stack is being put on screen: that one wins.
            tracing::debug!(render = ?render.id, "open: dropped, another stack is switched to");
            return;
        }
        let opened_as = self.renders.read(cx).open_generation();
        let stack = render.stack.clone();
        self.switch_to(&stack, None, window, cx, move |this, incoming, window, cx| {
            // Nothing kept and no render listed (the strip lags the store):
            // the working version is the render's, based on it.
            let incoming = incoming.unwrap_or_else(|| Working::of_render(&render));
            this.edit(incoming, window, cx);
            // Shown unless another open was asked for meanwhile.
            if this.renders.read(cx).open_generation() == opened_as {
                this.show_render_result(&render, presented, window, cx);
            }
        });
    }

    /// Put `stack`'s working version on screen, keeping the current one for
    /// its stack, then call `then` with it (`fallback` when `stack` has none,
    /// else `None`). Written on the store thread; the screen changes once
    /// written ([`ListSave::Switching`]), nothing else meanwhile
    /// ([`Self::is_processing`]). If the outgoing one can't be kept, that is
    /// said and nothing changes. Nothing while another switch writes.
    ///
    /// [`ListSave::Switching`]: super::list::ListSave::Switching
    pub(super) fn switch_to(
        &mut self,
        stack: &deptheos_app::library::history::StackId,
        fallback: Option<Working>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Option<Working>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.list_save.is_switching() {
            tracing::debug!(?stack, "switch: one is being written already");
            return;
        }
        let outgoing = self.working(cx);
        let renders = self.renders.read(cx);
        let store = renders.store().clone();
        let history = renders.renders().to_vec();
        let to = stack.clone();
        tracing::debug!(from = ?outgoing.stack, ?to, "switch: writing");
        let task = cx.spawn_in(window, async move |this, cx| {
            let switched =
                crate::work::store(move || store.switch_stack(&outgoing, &to, &history, fallback))
                    .await;
            this.update_in(cx, |this, window, cx| {
                let kept = match &switched {
                    Ok(Ok(incoming)) => incoming.clone(),
                    _ => None,
                };
                let switching =
                    std::mem::replace(&mut this.list_save, super::list::ListSave::Idle { kept });
                debug_assert!(switching.is_switching(), "only a switch's landing ends it");
                this.lock_renders(cx);
                let failed = |this: &mut Self,
                              e: &dyn deptheos_app::words::explain::Explain,
                              window: &mut Window,
                              cx: &mut Context<Self>| {
                    let lead = say!(cx, "history-not-switched");
                    let message = crate::notices::notice(cx.wording(), &lead, e);
                    this.post(Kind::Error, Subject::Alone, message, Vec::new(), window, cx);
                    // What is on screen stays, and is kept again.
                    this.save_list(cx);
                };
                match switched {
                    Ok(Ok(incoming)) => {
                        tracing::debug!("switch: written");
                        // A switch landed: reread the Library's stacks, as an
                        // open can land with it on screen.
                        this.renders.update(cx, |r, cx| r.refresh_stacks(cx));
                        then(this, incoming, window, cx);
                    }
                    Ok(Err(e)) => failed(this, &e, window, cx),
                    Err(panic) => failed(this, &std::io::Error::other(panic), window, cx),
                }
                cx.notify();
            })
            .ok();
        });
        self.list_save = super::list::ListSave::Switching { _task: task };
        self.lock_renders(cx);
        cx.notify();
    }

    /// Render Info (Stack ▸ Render Info, ⌘I) for the render shown.
    pub(crate) fn show_render_info(&mut self, cx: &mut Context<Self>) {
        if !self.on_stacking() {
            return;
        }
        self.renders.update(cx, |r, cx| {
            if let Some(id) = r.active(cx).cloned() {
                r.set_info(Some(id), cx);
            }
        });
    }

    /// Show a render's result. A comparison goes on: a pinned render is
    /// compared again unless now shown; a frame reloads against it.
    fn show_render_result(
        &mut self,
        render: &Render,
        presented: PresentedStack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PresentedStack { stack, display, map, reframe } = presented;
        let stacked = render.stacked().cloned().collect();
        let stack = Arc::new(stack);
        // A frame compared with the result before it: compared with this one.
        let viewer = self.viewer.read(cx);
        let frame = viewer.shown_file().filter(|_| viewer.mode().comparing().is_some());
        let shown = stack.clone();
        self.stacking.update(cx, |s, cx| s.restore(shown, render.id.clone(), stacked, cx));
        self.viewer.update(cx, |v, cx| v.show_result(display, map, Some(&stack), reframe, cx));
        self.log_shown(cx);
        // Painting carries over to the render opened, and so does Adjust.
        if self.is_retouching() {
            self.enter_paint(cx);
            return;
        }
        if self.adjusting().is_some() {
            self.adjust_shown(cx);
            return;
        }
        // Its paint is part of the result: shown once the worker has it.
        if let Some((stack, key)) = self.result_doc(cx) {
            self.paint.update(cx, |p, _| p.present(key, stack));
        }
        // A pinned render compares with each render shown until it is the
        // one shown; traded with it, the sides swap so neither moves (ADR-149).
        let renders = self.renders.read(cx);
        if let Some(compare) = renders.compare().cloned() {
            if renders.is_active(&compare, cx) {
                self.renders.update(cx, |r, cx| r.clear_compare(cx));
                self.view(ViewEvent::Look(Look::Result), window, cx);
            } else {
                let traded =
                    ViewEvent::Traded { shown: render.id.clone(), before: compare.clone() };
                self.view(traded, window, cx);
                self.compare_render(compare, window, cx);
            }
        } else if let Some(frame) = frame.filter(|f| self.files.read(cx).path(*f).is_some()) {
            self.compare_file(frame, window, cx);
        }
    }
}
