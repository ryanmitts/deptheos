//! The working version (`deptheos_app::library::stacks`): put in the list
//! and settings, re-based when its render is saved, reverted to a render,
//! and compared with the render on screen for the title, change line and
//! Revert.
//!
//! Showing a render changes only the picture; the list and settings always
//! hold the working version (ADR-142). The render on screen
//! ([`Renders::active`]) is the one stored state; a [`Showing`] is derived
//! from [`Snapshot`] where needed. Revert to #n is the one command taking a
//! render's frames and settings, asking only when it would lose unstacked
//! work.
//!
//! [`Renders::active`]: crate::renders::Renders::active

use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::{Render, RenderId};
use deptheos_app::library::stacks::Settings;
use deptheos_app::library::stacks::{Compared, Showing, Working};
use gpui::{App, Context, Window};

use super::AppView;
use crate::params::ChangeLine;
use crate::words::{Wording as _, say};

impl AppView {
    /// `render` was saved from a working version based on `from`: it becomes
    /// the base, unless a Revert gave another since. The list unlocks before
    /// the save lands, so the working version may be kept with another stack
    /// on screen by now.
    pub(super) fn rebase_on_saved(
        &mut self,
        render: &RenderId,
        from: Option<&RenderId>,
        cx: &mut Context<Self>,
    ) {
        let renders = self.renders.read(cx);
        let Some(saved) = renders.find(render) else { return };
        if saved.stack == self.stack {
            if self.base.as_ref() == from {
                self.base = Some(render.clone());
                self.save_list(cx);
            }
            return;
        }
        // A Library write, on the store thread like every other.
        let (store, saved, from) = (renders.store().clone(), saved.clone(), from.cloned());
        cx.spawn(async move |this, cx| {
            let rebased =
                crate::work::store(move || store.stacked_off_screen(&saved, from.as_ref())).await;
            match rebased {
                Ok(Ok(())) => {
                    this.update(cx, |this, cx| {
                        this.renders.update(cx, |r, cx| r.refresh_stacks(cx))
                    })
                    .ok();
                }
                Ok(Err(e)) => tracing::warn!("could not re-base the working version kept: {e}"),
                Err(e) => tracing::warn!("could not re-base the working version kept: {e}"),
            }
        })
        .detach();
    }

    /// Put `working` in the frame list and the settings.
    pub(super) fn edit(&mut self, working: Working, window: &mut Window, cx: &mut Context<Self>) {
        let Working { stack, base, list, settings } = working;
        (self.stack, self.base) = (stack, base);
        self.files.update(cx, |f, cx| f.set_list(list, window, cx));
        if let Some(Settings { config, options }) = settings {
            self.params.update(cx, |p, cx| p.restore(&config, &options, window, cx));
        }
        self.save_list(cx);
        cx.notify();
    }

    /// Revert to the render shown, as [`Self::revert_to`].
    pub(crate) fn revert_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = self.snapshot(cx);
        let Some(to) = now.showing().revert().map(|r| r.to.id.clone()) else { return };
        self.revert_to(&to, window, cx);
    }

    /// Revert to #n: `to`'s frames and settings become the working version
    /// and `to` the base, asking first (Cancel default) if unstacked work
    /// would go. Nothing when already equal, while running, retouching or in
    /// the Library.
    pub(crate) fn revert_to(&mut self, to: &RenderId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_revert(cx) {
            return;
        }
        let now = self.snapshot(cx);
        let Some(render) = self.renders.read(cx).find(to).filter(|r| r.stack == self.stack) else {
            return;
        };
        let showing = Showing::new(&now.working, Some(render), now.base.as_ref(), now.compared);
        let Some(revert) = showing.revert() else { return };
        match revert.alert(crate::words::of(cx)) {
            None => {
                let render = render.clone();
                self.revert_now(&render, window, cx);
            }
            Some(alert) => self.ask_revert(alert, render.id.clone(), window, cx),
        }
    }

    /// Revert may run: nothing runs, Retouch closed, Stacking on screen.
    fn can_revert(&self, cx: &App) -> bool {
        !self.is_processing(cx) && !self.paint.read(cx).is_open() && self.on_stacking()
    }

    /// Ask before a lossy Revert: destructive "Revert", and "Cancel" for
    /// Return and Esc. Revert checks again, as a save may land meanwhile.
    fn ask_revert(
        &mut self,
        alert: deptheos_app::library::stacks::RevertAlert,
        to: RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(render = ?to, "revert: asking");
        let this = cx.weak_entity();
        let deptheos_app::library::stacks::RevertAlert { title, body } = alert;
        let confirm = move |window: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| this.revert_confirmed(&to, window, cx)).ok();
        };
        let revert = say!(cx, "revert-alert-revert");
        crate::form::ask_destructive(title, body, revert, confirm, window, cx);
    }

    /// Confirmed: revert to `to` if the working version still differs.
    pub(super) fn revert_confirmed(
        &mut self,
        to: &RenderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_revert(cx) {
            return;
        }
        let now = self.snapshot(cx);
        let Some(render) = self.renders.read(cx).find(to).filter(|r| r.stack == self.stack) else {
            return;
        };
        let showing = Showing::new(&now.working, Some(render), now.base.as_ref(), now.compared);
        if showing.revert().is_some() {
            let render = render.clone();
            self.revert_now(&render, window, cx);
        }
    }

    /// Make `render` the working version and base, showing it if another is.
    fn revert_now(&mut self, render: &Render, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!(render = ?render.id, number = render.number, "revert: done");
        self.edit(Working::of_render(render), window, cx);
        if self.renders.read(cx).active(cx) != Some(&render.id) {
            self.renders.update(cx, |r, cx| r.open(&render.id, window, cx));
        }
    }

    /// The change line with its Revert, the base's settings (dots) and the
    /// render shown's (ticks).
    pub(super) fn compared_in(
        &self,
        now: &Snapshot,
        cx: &App,
    ) -> (Option<ChangeLine>, Option<Settings>, Option<Settings>) {
        let showing = now.showing();
        let words = crate::words::of(cx);
        let line = showing.change_line(words).map(|text| ChangeLine {
            text: text.into(),
            edited: matches!(showing, Showing::Base { .. }),
            revert: showing.revert().map(|r| r.number()),
            retouching: self.paint.read(cx).is_open(),
        });
        let settings = |r: &Render| Settings { config: r.config, options: r.options };
        (line, showing.base().map(settings), showing.shown().map(settings))
    }

    /// [`Self::compared_in`], read now.
    #[cfg(test)]
    pub(super) fn compared(&self, cx: &App) -> Option<ChangeLine> {
        self.compared_in(&self.snapshot(cx), cx).0
    }

    /// The title bar's words: name, render shown ("#3 · Depth · 44 of 46
    /// frames", or the working version's "Pyramid · 46 frames"), then
    /// "Edited" or "Not stacked yet".
    pub(crate) fn title_parts(&self, cx: &App) -> TitleParts {
        self.title_parts_in(&self.snapshot(cx), cx)
    }

    /// [`Self::title_parts`], from this frame's `now`.
    pub(crate) fn title_parts_in(&self, now: &Snapshot, cx: &App) -> TitleParts {
        let store = self.renders.read(cx);
        let working = &now.working;
        let stacked = store.has_renders(&self.stack);
        let named = store.stack_info(&self.stack).and_then(|i| i.name.clone());
        let name = if named.is_some() || stacked {
            store.given_stack_name(&self.stack)
        } else {
            // Never stacked: its first frame's folder, as a render would be.
            working.list.files.first().and_then(|f| f.parent()).map(file_label)
        };
        if let Some(number) =
            self.running_number(cx).filter(|_| self.stacking.read(cx).is_running())
        {
            let kind = self.params.read(cx).stacker_config(cx).kind();
            let n = self.files.read(cx).stack_input().1.len();
            let method = kind.label(crate::words::of(cx)).to_string();
            let what = say!(cx, "history-title-stacking", number = number, method = method, n = n);
            return TitleParts { name, what: Some(what), note: None };
        }
        let what = match &now.shown {
            Some(r) => {
                let w = cx.wording();
                let render = deptheos_app::library::history::render_name(
                    w.shared(),
                    r.number,
                    r.name.as_deref(),
                    false,
                );
                let (included, total) = (r.included.iter().filter(|i| **i).count(), r.frames.len());
                let frames = if included == total {
                    say!(w, "frames-count", n = total)
                } else {
                    say!(w, "render-frames-of", stacked = included, total = total)
                };
                let method = r.config.kind().label(w.shared()).to_string();
                Some(say!(
                    w,
                    "history-title-render",
                    render = render,
                    method = method,
                    frames = frames
                ))
            }
            None => {
                let shots = working.list.shots().len();
                let kind = self.params.read(cx).stacker_config(cx).kind();
                (shots > 0).then(|| {
                    let method = kind.label(crate::words::of(cx)).to_string();
                    say!(cx, "history-title-working", method = method, n = shots)
                })
            }
        };
        let note = if now.showing().is_edited() {
            Some(TitleNote::Edited(say!(cx, "history-title-edited")))
        } else if !stacked && !working.list.is_empty() {
            Some(TitleNote::NotStacked(say!(cx, "history-title-not-stacked")))
        } else {
            None
        };
        TitleParts { name, what, note }
    }

    /// Read once per frame for title, strip, settings line and menus, since
    /// each read clones the list.
    pub(crate) fn snapshot(&self, cx: &App) -> Snapshot {
        let working = self.working(cx);
        let base = self.base_render(cx);
        let renders = self.renders.read(cx);
        // Only a render of the stack on screen is compared with.
        let shown = renders
            .active(cx)
            .and_then(|id| renders.find(id))
            .filter(|r| r.stack == self.stack)
            .cloned();
        let decoder_choice = self.params.read(cx).has_backend_choice();
        let compared = crate::params::compared_settings(&working.list, decoder_choice);
        Snapshot { working, base, shown, compared }
    }

    /// The working version on screen for the strip
    /// ([`crate::renders::Renders::set_on_screen`]).
    pub(crate) fn on_screen_in(&self, now: &Snapshot, cx: &App) -> crate::renders::OnScreen {
        crate::renders::OnScreen {
            working: now.working.clone(),
            compared: now.compared,
            retouching: self.paint.read(cx).is_open(),
        }
    }

    /// Hand the strip the working version, on each change and as Retouch
    /// toggles, so a menu opened before the next frame reads it current.
    pub(crate) fn sync_on_screen(&self, cx: &mut Context<Self>) {
        let on_screen = self.on_screen_in(&self.snapshot(cx), cx);
        self.renders.update(cx, |r, _| r.set_on_screen(on_screen));
    }

    /// The working version's base, while it is in the history.
    pub(crate) fn base_render(&self, cx: &App) -> Option<Render> {
        let base = self.base.as_ref()?;
        self.renders.read(cx).find(base).cloned()
    }

    /// Log the working version against the render shown, as it changes.
    pub(super) fn log_shown(&self, cx: &App) {
        let now = self.snapshot(cx);
        tracing::debug!("shown: {}", now.showing());
    }
}

/// The working version as a frame reads it ([`AppView::snapshot`]).
pub(crate) struct Snapshot {
    pub(crate) working: Working,
    /// Its base, while it is in the history.
    pub(crate) base: Option<Render>,
    /// The render on screen, when it is one of the stack on screen's.
    pub(crate) shown: Option<Render>,
    /// What counts as a change on the desktop, for this list.
    pub(crate) compared: Compared,
}

impl Snapshot {
    /// The working version against the render shown, or the base when none
    /// of this stack is on screen: only while a stack just shown (or whose
    /// render was deleted) waits for its base to open, or after the list
    /// was emptied with nothing shown. Adding frames keeps the render shown.
    pub(crate) fn showing(&self) -> Showing<'_> {
        let shown = self.shown.as_ref().or(self.base.as_ref());
        Showing::new(&self.working, shown, self.base.as_ref(), self.compared)
    }
}

/// What the title bar says ([`AppView::title_parts`]).
pub(crate) struct TitleParts {
    /// The stack's name; none with nothing to stack.
    pub(crate) name: Option<String>,
    pub(crate) what: Option<String>,
    pub(crate) note: Option<TitleNote>,
}

/// The title block's last line, kept in a narrow window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TitleNote {
    /// "Edited", by the dot: it differs from its base.
    Edited(String),
    /// "Not stacked yet": frames with no render of their stack.
    NotStacked(String),
}
