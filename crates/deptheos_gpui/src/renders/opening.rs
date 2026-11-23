//! Opening a render on the pool, with a spinner if slow. Opens share one
//! latest-wins slot ("render-open", ADR-143): a newer open or a run drops
//! the task, canceling its work, which checks its token between loading and
//! presenting, so a replaced 100 MP open stops there.
use deptheos_app::library::history::RenderId;
use gpui::{Context, Window};

use super::{Open, OpenedRender, Renders, RendersEvent, SPINNER_DELAY};
use crate::words::{Wording as _, say};
use crate::work;

impl Renders {
    /// Load a render in the background, to show it; [`RendersEvent::Opened`]
    /// follows. The working version is never changed by it.
    pub fn open(&mut self, id: &RenderId, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_locked() {
            return;
        }
        let store = self.store.clone();
        let Some(render) = self.find(id).cloned() else { return };
        self.generation += 1;
        let generation = self.generation;
        let engine = self.engine.clone();
        let words = cx.wording().clone();
        let failed = say!(cx, "render-open-failed");
        let load = cx.spawn_in(window, async move |this, cx| {
            let loaded = work::interactive("render-open", move || {
                let stack = store.load_render(&render.id).map_err(deptheos_app::Error::Store)?;
                work::current().check()?;
                let presented = engine.present_stack(stack)?;
                Ok::<_, deptheos_app::Error>(OpenedRender { render, presented })
            })
            .await;
            let loaded = crate::notices::worked(&words, &failed, loaded);
            this.update(cx, |this, cx| {
                // Still the open in flight? Leaving `Loading` ends it.
                if !this.end_open(generation) {
                    return;
                }
                match loaded {
                    Ok(opened) => {
                        // The app shows it: `restore` makes it the result.
                        this.opened = Some(opened);
                        cx.emit(RendersEvent::Opened);
                    }
                    Err(why) => this.fail(why, cx),
                }
                cx.notify();
            })
            .ok();
        });
        let spinner = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SPINNER_DELAY).await;
            this.update(cx, |this, cx| {
                if let Open::Loading { generation: current, slow, .. } = &mut this.open
                    && *current == generation
                {
                    *slow = true;
                    cx.notify();
                }
            })
            .ok();
        });
        tracing::debug!(render = ?id, generation, "open: loading");
        self.open = Open::Loading {
            id: id.clone(),
            generation,
            slow: false,
            _load: load,
            _spinner: spinner,
        };
        cx.notify();
    }

    /// Open `generation` has loaded: back to idle. `false` if it is no
    /// longer the open in flight.
    fn end_open(&mut self, generation: u64) -> bool {
        match &self.open {
            Open::Loading { generation: current, .. } if *current == generation => {}
            _ => return false,
        }
        self.open = Open::Idle;
        tracing::debug!(generation, "open: loaded");
        true
    }

    /// Numbers the opens asked for: changed, a newer one was asked for.
    pub fn open_generation(&self) -> u64 {
        self.generation
    }

    /// An open is in flight.
    pub fn is_pending(&self) -> bool {
        matches!(self.open, Open::Loading { .. })
    }

    /// The render the open in flight is loading.
    pub(crate) fn pending_id(&self) -> Option<&RenderId> {
        match &self.open {
            Open::Loading { id, .. } => Some(id),
            Open::Idle => None,
        }
    }

    /// The open in flight has taken long enough to show a spinner.
    pub(super) fn open_is_slow(&self) -> bool {
        matches!(self.open, Open::Loading { slow: true, .. })
    }

    /// Drop the open in flight, and a render loaded but not yet taken.
    pub(crate) fn cancel_open(&mut self) {
        if self.is_pending() {
            tracing::debug!("open: canceled");
        }
        self.open = Open::Idle;
        self.opened = None;
    }
}
